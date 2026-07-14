# Design Spec: Notifications + Notification Center

> *"Built for people who care about how things feel."* — RaeenOS_Concept.md
>
> System events surface as quiet, beautiful toasts — never a modal interruption,
> never a mystery beep — and the ones you miss wait for you in a calm, grouped
> pull-down. It must clear: **macOS Sequoia/26's grouped Notification Center with
> per-app stacking + inline actions, and Windows 11 24H2's Action Center
> notification list with Focus assist** — without Win11's vanishing-toast amnesia
> or macOS's two-surface (NC vs Control Center) split confusion.

**All tokens below are defined in [`design-language.md`](./design-language.md)**
and live in the `rae_tokens` crate (ADR 0003). This spec only assigns them; it
introduces no new magic numbers. Toast geometry (320×72) and panel width (380px)
are **local layout constants** for the kernel-drawn surface, expressed as
`space.*`/token multiples — deliberately not new global tokens.

This spec **owns the notification system end to end** and supersedes the toast
sketch in [`desktop-shell.md`](./desktop-shell.md) §5 and the "future
notification-center history panel (not in scope here)" deferral in
[`control-center.md`](./control-center.md) §3. Both now defer here.

---

## Concept promise + bar to clear

> "system events surface as quiet, beautiful toasts — never a modal interruption,
> never a mystery beep" + "The user owns the machine" (no nag, no ads).
> — RaeenOS_Concept.md (§RaeShell, §"The user owns the machine")

- **Bar to clear:**
  - **macOS Sequoia/26 Notification Center** — toasts slide in top-right, then
    *collapse into a per-app stack* you expand on hover; the Center is a scrollable
    column of grouped cards; notifications carry **inline actions** (Reply, Mark as
    read, snooze) without launching the app; Focus modes silence by category.
  - **Windows 11 24H2 Action Center** — a list of grouped notifications with
    per-app collapse, "Clear all," inline quick-reply on messaging toasts, and
    Focus assist (priority-only / alarms-only).
- **The RaeenOS-specific promise — the #1 parity gap (memory `goal-rival`):**
  - **No vanishing-toast amnesia.** Win11's #1 pain: a toast you glance away from
    is gone forever. RaeenOS **retains every post in a history ring after its TTL**
    (already built — see below), so the Center shows exactly what you missed.
  - **One coherent right-edge model**, not two flyouts: toasts (top-right) and the
    Notification Center pull-down share material/accent/elevation with Control
    Center (bottom-right) — same glass, one mental model (`control-center.md` §3).

---

## Already built (delta only — verify-before-spec)

Grounded in `kernel/src/notify.rs` (read, not assumed). **The implementation is
ahead of the design here** — this spec codifies what exists and specs the polish
delta (grouping visuals, inline actions, toast→center continuity, motion), so the
next polish wave does not drift the built surface out of the design language.

| Piece | Where (`kernel/src/notify.rs`) | Today | This spec adds |
|---|---|---|---|
| Toasts | `post` / `post_at`, `TOAST_W=320`, `TOAST_H=72`, `MAX_VISIBLE=3`, `TOAST_TTL_MS=5000` | LIVE, glass tokens already wired (`GLASS_TINT_DARK`, `RADIUS_MD`, `stroke_strong` top edge, urgency bar via state tokens) | the **stack depth cue** (96% scale + dim per card back), per-app collapse, the §5 inline-action row |
| History ring | `HISTORY` `Vec<HistoryEntry>`, `HISTORY_CAP=64`, `record_history` (grouped by source), `dismiss_history`, `clear_history` | LIVE — retains every post after TTL, grouped by source | the **grouped-card visual** + empty state + clear-all motion (§3) |
| Notification Center | `toggle_center`, `CenterPanel`, `render_center`, `CENTER_W=380`, scrollable grouped list, dismiss-one / clear-all, over a quick-settings strip | LIVE (kernel-drawn pull-down) | the full state matrix, group-collapse interaction, dark/light parity, motion tokens, controller nav |
| Focus / Do-Not-Disturb | `quick_settings::dnd_enabled()` suppresses the toast but **keeps history** | LIVE & correct (DND silences interruption, never loses the message) | the **DND visual state** (tray badge + a "Delivered Quietly" section in the Center, the macOS model) |
| Quick-settings strip in Center | `quick_settings` (5 real-backend toggles: Wi-Fi, mute, DND, Night Light, Vibe accent) | LIVE | defers to [`control-center.md`](./control-center.md) for the *full* Control Center; the in-Center strip stays a compact summary (see §6) |
| Glass / shadow / accent | `CARD_BG=GLASS_TINT_DARK`, `proof_accent()` = `derive_accent(active_accent).base` | LIVE | inherits the soft-shadow fix from [`material-and-shadow.md`](./material-and-shadow.md) (toasts/Center must not show the hard-block shadow) |

**Net delta to build:** (1) toast stack depth-cue + per-app collapse; (2) the
grouped-card *visuals* over the existing grouped data; (3) the inline-action row
(§5); (4) DND visual state; (5) motion/material tokenization to match the design
language; (6) controller/couch nav. **Not a rebuild** — the engine, history ring,
grouping data, DND, and the Center panel all exist.

---

## Prior art distilled (current systems, 2024–2025)

- **macOS Sequoia/26:** toasts land top-right and *coalesce into a per-app stack*
  (a fanned deck you click to expand); the Notification Center is a single
  scrollable glass column reached by clicking the clock / two-finger swipe from the
  right; cards carry **inline actions** (Reply field on Messages, snooze on
  reminders) handled without foregrounding the app; Focus modes route by category
  and show a "Delivered Quietly" section for the suppressed ones. **Take:** per-app
  stacking, inline actions, the "delivered quietly" honesty, one glass column.
  **Avoid:** the *two separate surfaces* (NC pull-down vs Control Center) the user
  must learn — RaeenOS keeps both right-edge with one material and an adjacent
  mental model (`control-center.md` §3).
- **Windows 11 24H2:** Action Center pull-up groups notifications by app with a
  collapse chevron, "Clear all" per group and global, inline quick-reply on
  messaging apps, and Focus assist tiers. **Take:** per-app group chevron, the dual
  clear-all (per-group + global), quick-reply. **Avoid:** the *vanishing toast* (no
  retention until you open Action Center — RaeenOS retains from the moment of post);
  the calendar/clock occupying half the flyout (RaeenOS Center is notifications-first).
- **GNOME 46+:** a single calendar+notifications popover, grouped by app, "Clear"
  per group; minimal but coherent. **Take:** the calm, low-chrome grouped list.
  **Avoid:** the cramped popover width on large displays.

**RaeenOS synthesis:** macOS's **per-app stacking + inline actions + "delivered
quietly"** honesty, Win11's **per-group + global clear-all**, on one right-edge
glass material shared with Control Center, with **zero-amnesia retention** as the
differentiator — every post is kept the instant it fires.

---

## RaeenOS design tokens this surface uses

Pulled verbatim from `design-language.md` / `rae_tokens`. No new magic numbers.

- **spacing:** `space.2` (toast stack gap — already `TOAST_GAP`, card internal gap),
  `space.3` (card vertical padding, group-header inset), `space.4` (panel padding,
  edge margin — already `TOAST_MARGIN`), `space.5` (section gap inside the Center).
- **radius:** `radius.md` (toast + Center cards — already `TOAST_RADIUS`),
  `radius.lg` (the Center panel itself), `radius.xs` (inline-action buttons,
  dismiss × hit area, group chevron), `radius.pill` (the snooze/quick-reply chip).
- **elevation:** `elev.2` (a single resting toast, a Center card lift),
  `elev.3` (the Center pull-down panel — transient glass over the desktop),
  `elev.focus` (focused card / focused inline-action glow). **All inherit the
  soft-shadow contract** from `material-and-shadow.md` — never the hard block.
- **type:** `type.label` (toast/card title, group-app name), `type.body`
  (notification body when expanded), `type.caption` (source + relative time +
  group count badge + "Delivered Quietly" header + inline-action labels).
- **accent model:** seed = `ThemeAbi.accent_argb`; `rae_tokens::derive_accent`.
  Normal-urgency bar = `accent.base` (already `proof_accent()`); inline-action
  primary button = `accent.subtle` fill + `accent.text`; focus = `accent.base`
  ring + `accent.glow`. Critical = `state.danger`, low = `text.tertiary`. **No
  private `const ACCENT`** (already satisfied — `proof_accent()` reads the live
  seed so a Vibe re-skin recolours toasts with the shell).
- **material:** `material.glass` — toasts and the Center are small, transient,
  right-edge surfaces: exactly the Mica/Acrylic doctrine's case for live blur.
  `GLASS_TINT_DARK` / `GLASS_TINT_LIGHT`, 1px `stroke.strong` top-edge highlight,
  1px `stroke.subtle` other edges.
- **motion:** `motion.fast` (toast slide-in from right + fade; Center open
  pull-down), `motion.exit` (toast auto-dismiss / Center close — ~40% faster than
  entry), `motion.micro` (hover, dismiss ×, group expand/collapse, inline-action
  press), `motion.standard` (per-app stack fan-out on hover), `motion.instant`
  (reduced-motion fallback).

---

## 1. Toasts (transient, top-right)

**Bar to clear:** macOS toast landing + per-app stacking.

### Geometry (already built — tokenized here)
- 320×72px card, top-right, anchored `space.4` (`TOAST_MARGIN`) from top + right.
- `material.glass`, `radius.md`, 1px `stroke.strong` top edge, `elev.2` (soft).
- Up to `MAX_VISIBLE=3` stacked, `space.2` (`TOAST_GAP`) vertical gap, newest on
  top, oldest evicted (already implemented).
- **Stack depth cue (delta):** each card *behind* the front one renders at 96%
  scale (centered) and ~8% dimmer (composite alpha), so a stack reads as depth,
  not a flat list — the macOS deck cue. The front (newest) card is full size.

### Anatomy (left → right)
1. **Urgency bar** (left 4px, `radius` left corners only): low → `text.tertiary`,
   normal → `accent.base`, critical → `state.danger` (already mapped).
2. **App/source icon** (28px, `space.3` inset) — from the posting source.
3. **Title** (`type.label`, `text.primary`, 1-line clamp) + **source · time**
   subrow (`type.caption`, `text.tertiary`).
4. **Dismiss ×** (top-right, 24px hit area `radius.xs`, appears on hover only).

### Per-app collapse (delta)
- When ≥2 unexpired toasts share a source, they collapse into **one** front card
  with a **count badge** (`type.caption` `accent.subtle` pill, e.g. "+2"). Hover
  the badge → `motion.standard` fan-out into the individual cards (still ≤
  `MAX_VISIBLE` on screen). Click a fanned card → activate that source.

### Inline actions on toasts (delta — see §5)
- If a notification declares actions, the toast grows a single action row at the
  bottom (one beat after landing, `motion.micro`) — at most 2 buttons + an
  overflow into the Center. Keeps the toast a glance surface; deep actions live in
  the Center.

### States (per toast)
- **default:** glass over desktop, soft `elev.2`.
- **hover:** dismiss × + action row appear; the **auto-dismiss timer pauses**
  (already a timer in `expire_tick`); `motion.micro`.
- **active (click body):** brief `accent.active` flash → activate source → dismiss
  on `motion.exit`.
- **focus (keyboard, `Super+N` to focus the stack):** 2px `accent.base` ring +
  `elev.focus`; arrow keys move within the stack, `Enter` activates, `Del`
  dismisses.
- **critical:** `state.danger` bar; does **not** auto-dismiss (sticks until acted
  on); is **not** suppressed by DND (already the `notify.rs` contract).
- **dark/light:** `GLASS_TINT_DARK` / `GLASS_TINT_LIGHT` + palette text tokens.
- **reduced-motion:** appear/disappear instantly, no slide/scale; the stack
  depth-cue becomes a flat 1px `stroke.subtle` separator instead of scale/dim.

### Never steals focus
Already the contract — a toast is an overlay, never a modal, never grabs keyboard
focus from a running game (Concept §RaeShell).

---

## 2. Notification → Center continuity (the zero-amnesia promise)

The single most important feel detail: **a toast you ignore is not lost.** On TTL
expiry (or manual dismiss without activation), the card is already `record_history`'d
into the ring. The *visual* continuity to add:

- When a toast expires, it does **not** just fade — it slides up-and-right a few px
  and fades on `motion.exit`, telegraphing "it went to the Center." (Reduced-motion:
  plain fade.)
- The tray clock / notification glyph shows an **unread count badge**
  (`accent.subtle` pill, `type.caption`) so the user knows the Center has unseen
  items — the antidote to Win11 amnesia. Badge clears when the Center is opened.

---

## 3. Notification Center (the pull-down)

**Bar to clear:** macOS NC grouped column + Win11 Action Center clear-all.
**Already built** (`toggle_center` / `render_center`, `CENTER_W=380`) — this specs
the visuals + state matrix.

### Geometry
- Panel width **380px** (`CENTER_W`), height = screen − taskbar − `space.4`,
  anchored **top-right** sliding down from the tray clock at `space.4` inset.
- `material.glass` (`radius.lg`, blur 16, tint per palette, 1px `stroke.strong`
  top edge, `stroke.subtle` border), `elev.3` (soft).
- Content padding `space.4`; `space.5` between sections.
- **Open:** pull-down — translate from −16px + fade 0→1, `motion.fast`. **Close:**
  `motion.exit`. Click-away or `Esc` closes. Never steals game focus.

### Layout (top → bottom)
1. **Header row** — "Notifications" (`type.subtitle` `text.primary`) left; a
   **Clear all** text button (`type.label` `accent.text`, `radius.xs` hover) right.
   When DND is on, a `type.caption` `text.tertiary` "Focus on" badge sits beside
   the title.
2. **Grouped history list** (scrollable) — one **group card per source**
   (`record_history` already groups by source), newest group first:
   - **Group header:** app icon (24px) + app name (`type.label`) + a count badge
     (`type.caption` `accent.subtle` pill) + a **collapse chevron** (right) + a
     per-group **Clear** (× on hover).
   - **Stacked items:** newest item shown expanded; older items of the same group
     collapse behind it as a deck (96% scale offset, like the toast stack) with the
     count badge. Click the header/chevron → `motion.micro` expand to show all
     items as individual rows; collapse again to re-deck.
   - **Item row:** title (`type.label` `text.primary`), body (`type.body`
     `text.secondary`, 2-line clamp), source·time (`type.caption` `text.tertiary`),
     a dismiss × (`radius.xs`, hover), and the inline-action row (§5) when declared.
3. **"Delivered Quietly" section (delta — the macOS honesty model):** when DND is
   on, notifications suppressed-as-toast-but-kept appear here under a
   `type.caption` `text.tertiary` "Delivered Quietly" header, visually calmer
   (no urgency bar). This makes DND legible: you silenced the *interruption*, not
   the *message*.
4. **Quick-settings summary strip** (existing `quick_settings`) — see §6.

### States
- **group hover:** group card `bg.elevated` lift; chevron + Clear × reveal,
  `motion.micro`.
- **group expand/collapse:** `motion.micro` deck↔list; one group's expansion does
  not force-collapse others (independent).
- **item hover:** dismiss × + action row reveal.
- **item dismiss:** slide-out-right + collapse the gap, `motion.exit`; group count
  decrements; group disappears when empty.
- **clear all:** all groups slide-out-right staggered ~30ms apart (`motion.exit`),
  then the empty state (§4) cross-fades in.
- **focus (keyboard/controller):** Tab/arrows move group→item→action; focused
  element = 2px `accent.base` ring + `elev.focus` (never color-only). `Enter`
  activates, `Del` dismisses, `←/→` collapse/expand a group.
- **dark/light:** glass tint + text tokens per palette; soft-shadow alpha ×0.6 in
  light (design-language §5.3).
- **reduced-motion:** no slides/decks — items appear/disappear instantly; clear-all
  is an instant cut to the empty state; groups show as flat lists with `stroke.subtle`
  separators instead of scaled decks.

---

## 4. Empty / no-notifications state

- A centered calm block: a soft `text.tertiary` bell-off glyph + `type.body`
  `text.secondary` "You're all caught up" + `type.caption` `text.tertiary`
  "Notifications you receive will appear here." No blank rows, no fake skeleton.
- The quick-settings strip (§6) remains visible below — the panel is never *fully*
  empty, so it doesn't read as broken.

---

## 5. Inline actions (the macOS/Win11 action parity — delta)

A notification source may declare up to **3 actions**; the surface renders them
without launching the app (Concept "fast is a feature").

| Action kind | Render | Behavior |
|---|---|---|
| **Button** (e.g. "Mark read", "Accept") | `radius.xs` chip, `type.caption`; primary = `accent.subtle` fill + `accent.text`, secondary = `bg.elevated` + `text.primary` | fires the source's callback (capability-gated), then dismisses the item |
| **Quick-reply** (messaging) | a `radius.pill` text field + send glyph appears inline on activation | text goes to the source via the notification IPC channel; field uses `Canvas::draw_text` |
| **Snooze** | `radius.pill` chip with a small duration popover (15m / 1h / tomorrow) | re-posts the notification after the interval (uses the existing TTL/time machinery) |

- On a **toast**: at most 2 buttons inline + overflow ("More" → opens the Center to
  that item). On a **Center item**: full action row.
- **`Cap`-gating:** every action callback routes through the source's
  capability-checked IPC — the notification surface never bypasses RaeShield. A
  source without the action capability simply shows no action row.
- **States:** button hover `bg.overlay`/`accent.subtle` lighter; press
  `accent.active` flash; focus ring + glow; disabled (no cap) → not rendered.
- **reduced-motion:** the action row appears instantly (no grow), reply field
  appears with no slide.
- **Flag to raeen-accessibility:** quick-reply must be fully keyboard-reachable and
  the send action must have a non-pointer trigger (`Enter` in the field).

---

## 6. Relationship to Control Center (one model, two surfaces)

Per `control-center.md` §3: notifications (top-right) and Control Center
(bottom-right) are deliberately **adjacent, not merged**, sharing material/accent/
`elev.3`. The Notification Center carries a **compact quick-settings strip**
(`quick_settings`'s 5 real-backend toggles) at its bottom for one-reach access, but
the **full** toggle grid, sliders, media card, and Gaming row live in Control
Center. This spec owns the strip's *summary* rendering; `control-center.md` owns
the full flyout. They must read the **same accent and glass** so the user perceives
one system, two docks.

---

## 7. Accessibility (in scope from the start)

| Concern | Rule | Owner |
|---|---|---|
| Contrast | toast/card title `type.label` ≥7:1; body ≥4.5:1; time/source ≥3:1 (non-essential); urgency-bar colors must clear 3:1 against the glass | raeen-accessibility |
| Focus visibility | every card/group/inline-action is **never color-only**: `accent.base` ring + `elev.focus` glow | raeen-accessibility |
| Reduced-motion | no slides/decks/fan-out; instant appear/dismiss; clear-all is a cut | raeen-accessibility |
| Hit targets | dismiss × ≥32px pointer, action chips ≥32px; couch ≥48px | raeen-visual-qa |
| Keyboard-complete | open Center, move group→item→action, activate, dismiss, clear-all, quick-reply — all with no pointer | raeen-accessibility |
| DND legibility | DND state visible (tray badge + "Delivered Quietly" section), never a silent black hole | raeen-accessibility |

Flag to **raeen-accessibility:** confirm the unread-count badge and urgency bar
stay legible when the accent is re-seeded via Vibe Mode (low-saturation accents
must keep a 3:1 contrast or fall back to `text.primary`).

---

## 8. Cohesion acceptance (the whole-surface test)

Ships only when:
1. **Same accent:** toast normal-urgency bar, unread badge, inline-action primary,
   and the Center read the *same* `accent.base` as the taskbar / Start / Control
   Center / Files — switch one Vibe preset and they all recolor together.
2. **Same material/radii/shadow:** glass `radius.md` cards + `radius.lg` panel +
   *soft* `elev.*` (post `material-and-shadow.md` fix) identical to Control Center.
3. **Zero amnesia proven:** a toast that expires is in the Center.
4. **DND honest:** suppressed toasts appear in "Delivered Quietly," never lost.
5. **Dark + light parity** with passing contrast.

---

## Handoff

### Implementer
- **kernel (`notify.rs`) — primary owner.** The notification system is kernel-drawn
  (mirrors `SettingsPanel`/Center already in `notify.rs`). Build the delta there:
  toast stack depth-cue + per-app collapse, the grouped-card *visuals* over the
  existing grouped `HISTORY` data, the "Delivered Quietly" section under DND, the
  toast-expiry→Center slide continuity, the unread-count badge signal to the tray,
  and the motion tokenization. Keep all rendering under `lock_compositor()` with the
  IF=0 → unlock-before-scanout discipline (memory `compositor-IF=0`); no per-frame
  allocations on the toast/center paint path (memory `iron-console-logging-tax`).
- **raeen-shell-apps** — the **inline-action IPC + dispatch** (the notification
  source declares actions over its capability-checked channel; the shell routes
  the button/quick-reply/snooze callbacks). The tray unread badge is rendered by
  the shell's tray cluster (see `system-tray.md`).
- **raeen-ui** — expose the **notification-card widget** (icon + title + body +
  time + dismiss + action row) and the **group-deck container** as reusable
  `rae_tokens`-consuming widgets so the toast and the Center share one renderer.
- **raeen-accessibility (flagged)** — quick-reply keyboard path; DND legibility;
  badge/urgency contrast under re-seeded accents; reduced-motion audit.

### FAIL-able boot-log proof lines
Extend the existing `notify::run_boot_smoketest` (which already drives synthetic
time). Each must be able to print FAIL:

```
[notify] toast smoketest: posted=4 visible=3 evicted_oldest=1 stack_depthcue=1 -> PASS
[notify] history smoketest: expired=1 retained_in_center=1 (zero-amnesia) -> PASS
[notify] group smoketest: source=Mail items=3 collapsed=1 count_badge=+2 -> PASS
[notify] dnd smoketest: posted_under_dnd=1 toast_suppressed=1 in_delivered_quietly=1 critical_breaks_dnd=1 -> PASS
[notify] action smoketest: declared=2 rendered=2 nocap_action_hidden=1 cap_dispatch_ok=1 -> PASS
[notify] accent smoketest: bar=0x.. == derive_accent(active_accent).base -> PASS
```

(FAIL if an expired toast is absent from history, if a suppressed-under-DND item
is missing from "Delivered Quietly," if a critical notification is suppressed by
DND, if an action without its capability still renders, or if the bar color
diverges from the live accent.)

### Visual-QA verification list (raeen-visual-qa)
Verify on iron / host-render / QEMU window (headless screendump striping is a
capture artifact, not the render — memory `ui-glass-design-system`):
- Screenshot: 3 stacked glass toasts top-right — depth cue visible (back cards
  smaller + dimmer), urgency bars (normal=accent, critical=danger), soft shadow
  (NOT a hard offset block — the `material-and-shadow.md` penumbra test).
- Screenshot: a toast with the +N count badge, and the fanned-out state on hover.
- Screenshot: a toast with an inline-action row (2 buttons, primary = accent).
- Screenshot: the Notification Center open — grouped cards per source with count
  badges + collapse chevrons + a Clear-all header; soft `elev.3`; glass blur.
- Screenshot: a group expanded (deck → individual rows) vs collapsed.
- Screenshot: the **empty** "You're all caught up" state.
- Screenshot: DND on — tray badge + the "Delivered Quietly" section populated.
- Cohesion: before/after one Vibe preset switch — toast bar + Center accent change
  *with* the taskbar/Control Center in the same frame.
- Reduced-motion on: no slides/decks; instant appear/dismiss.

### Unblocks (MasterChecklist)
- **Phase 14.1 (Notifications surface):** from toast-only to a full grouped Center
  with inline actions + DND + zero-amnesia — the parity-#1 gap.
- **Phase 8 (RaeUI/RaeKit):** the notification-card + group-deck reusable widgets.
- **Phase 13 (Customization):** accent-coherent toasts/Center make the Vibe re-skin
  reach the notification surface end to end.
