# Design Spec: Desktop Shell

> *"Built for people who care about how things feel."* — LEGACY_GAMING_CONCEPT.md
>
> The shell is the first thing the user sees after login and the surface they
> touch most. It must clear: **the familiarity of Windows 11's taskbar+Start,
> the cleanness of macOS's chrome, the cool factor of a Linux that's actually
> coherent.**

**All tokens below are defined in [`design-language.md`](./design-language.md).**
This spec only assigns them; it introduces no new magic numbers.

---

## Already built (delta only — verify-before-spec)

| Piece | Where | Today | This spec changes |
|---|---|---|---|
| Taskbar | **LIVE: `components/athshell/src/lib.rs::DesktopShell::render`** (`TASKBAR_HEIGHT=36`, flat `BAR_BG 0x0A0E1A`, `ACCENT 0x4E9CFF`). *(`athshell/src/desktop.rs` `TASKBAR_H=40` is DEAD code — never instantiated; do not touch.)* | flat, hardcoded palette | → 44px, `material.mica`, `radius` on the Start pill + task buttons, hover/active/focus states |
| Window chrome | `kernel/src/window_chrome.rs` (`TITLE_BAR_H=28`, flat `CHROME_BG`, block-font glyphs, square min/max/close) | functional, pre-polish | → 32px, focused/unfocused tint, real macOS-order traffic-light controls w/ hover, draggable, accent focus stroke |
| Start menu | `athshell::StartMenu` (kernel-drawn, app list, pinned/categories) | exists | → `material.glass` panel, `radius.lg`, search field, recents grid, motion-in |
| System tray / clock | `athshell::SystemTray` (`tray_clock_string`) | clock only | → tray icon cluster + quick-settings flyout |
| Notifications/toasts | `kernel/src/notify.rs` (320x72, max 3, oldest-evicted, urgency bar) | LIVE & good | → stack spacing/motion tokens, glass material, accent urgency bar |
| Drop shadow / blur / HDR | `compositor.rs` | LIVE | reused as `elev.*` / `material.glass` |

The shell is **not a rebuild** — it is a re-skin onto existing surfaces plus
state-completeness (hover/active/focus/disabled, dark/light, reduced-motion).

---

## 1. Taskbar (bottom, Windows-familiar layout)

**Bar to clear:** Windows 11 taskbar (centered-or-left apps, Start, tray) +
macOS Dock magnification feel. AthenaOS keeps Windows' *spatial* model (bottom,
Start left, tray right) and macOS's *materiality* (glass/mica, rounded).

### Geometry
- Height: **44px** (`space.4`+`space.5`-ish; up from 40 for the 32px target
  floor + padding). Full width, docked bottom.
- Material: `material.mica` (static, wallpaper-tinted — off the per-frame blur
  path). Shadow `elev.1` cast upward (`offset_y = -1`, radius 6).
- Top edge: 1px `stroke.strong` highlight (the glass top-edge rule), NOT the
  current full-saturation accent line.
- Inner padding: `space.2` (8px) left/right; items vertically centered.

### Regions (left → right)
1. **Start pill** — `radius.pill`, height 32px, `space.3` horizontal padding,
   label "Rae" or logo glyph in `accent.text`, `type.label`. Left-aligned at
   `space.2`.
2. **Pinned + running apps** — icon buttons, 36px square, `radius.sm`, `space.1`
   gaps. Running apps show a 3px `accent.base` indicator bar centered under the
   icon (Windows 11 pattern); focused app's indicator is full-width
   `accent.base`, unfocused running = 12px stub `text.tertiary`.
3. **Tray cluster** (right) — status icons (network, volume, battery) at 32px
   hit targets, `space.1` gaps, then the **clock** (`type.caption`, two lines
   time/date) right-aligned at `space.2`.

### States (per app/tray button)
- **default:** transparent over mica.
- **hover:** `bg.elevated` fill @ `radius.sm`, `motion.micro` fade-in.
- **active (pressed):** `accent.subtle` fill.
- **focused (running, has window focus):** full-width `accent.base` indicator +
  `bg.elevated` fill.
- **keyboard/controller focus:** 2px `accent.base` ring + `elev.focus` glow.
- **disabled:** `text.tertiary`, no hover.
- **dark/light:** mica + tokens per palette.
- **reduced-motion:** state changes are instant; no slide.

### Interaction & keyboard/controller map
- Click Start pill → toggle Start menu. `Super` key → same.
- Hover app button → tooltip (`material.glass` mini, `type.caption`) after 500ms.
- Click running app → focus/restore; click focused → minimize.
- Keyboard: `Super`=Start, `Super+1..9`=launch/focus pinned N, `Super+B`=focus
  tray cluster then arrow-keys between icons, `Enter`=activate.
- Controller (couch): not the taskbar's job — couch mode is a separate surface
  (future `couch.md`); desktop taskbar targets stay ≥32px so a trackpad/stick
  cursor still works.

---

## 2. Start / Launcher

**Bar to clear:** Windows 11 Start (pinned grid + recents + search) with macOS
Spotlight's instant fuzzy search feel.

### Geometry
- Panel: width **560px**, height **640px** (clamped to screen − taskbar −
  `space.4`), anchored bottom-left above the Start pill at `space.2` inset.
- Material: `material.glass` (`radius.lg`, blur 16, tint per palette, top-edge
  highlight, `stroke.subtle` border). Shadow `elev.3`.
- Content padding: `space.4` (16px).

### Layout (top → bottom)
1. **Search field** — full width, height 40px, `radius.sm`, `bg.elevated` fill,
   search glyph + placeholder "Search apps, files, settings" (`text.tertiary`,
   `type.body`). Focus on open. Typing filters the grid live (fuzzy).
2. **Pinned grid** — 6 columns, icon 48px + label below (`type.caption`,
   `text.secondary`), cell `radius.sm`, `space.3` gaps. Section header "Pinned"
   `type.subtitle`.
3. **Recents / suggested** — 2 rows of recent apps/files, same cell style.
4. **Footer bar** — user avatar + name (`type.label`) left; power/settings
   glyphs right (32px targets, `radius.xs` hover).

### States
- **app cell hover:** `bg.elevated` @ `radius.sm`, `motion.micro`.
- **app cell active:** `accent.subtle`.
- **keyboard/controller focus:** 2px `accent.base` ring + `elev.focus` glow;
  arrow keys move focus across the grid, `Enter` launches, `Esc` closes.
- **open animation:** scale 96%→100% + fade 0→1 + 8px upward translate,
  `motion.standard`.
- **close:** `motion.exit` (fade + scale to 96%).
- **reduced-motion:** instant show/hide, opacity only.
- **dark/light:** glass tint + text tokens per palette.

---

## 3. System tray / Quick-settings flyout

**Bar to clear:** Windows 11 Quick Settings + macOS Control Center.

### Tray icons (in the taskbar tray cluster)
- Network, volume, battery (when present), plus dynamic (capture/recording =
  `state.danger` dot, per `capture.rs`). 32px targets, `text.secondary` resting,
  `text.primary` on hover.

### Quick-settings flyout
- Trigger: click the tray cluster (or `Super+A`).
- Panel: width **360px**, anchored bottom-right above the tray at `space.2`.
  `material.glass`, `radius.lg`, `elev.3`.
- Content:
  - **Toggle tiles** — 2-column grid, each 160×64px, `radius.md`, `bg.elevated`
    when off / `accent.subtle` + `accent.text` icon when on. (Wi-Fi, Bluetooth,
    Night Light, Game Mode, Do-Not-Disturb, Airplane.)
  - **Sliders** — volume + brightness, full width, 8px track `radius.pill`,
    filled portion `accent.base`, knob 16px `radius.pill` with `elev.2`.
  - **Footer:** quick link to Settings + power.
- States: tile hover `bg.elevated`→ lighter; tile on = `accent.subtle`; focus
  ring as §1. Open/close motion = `motion.fast` (smaller surface than Start).

---

## 4. Window chrome (titlebar + controls)

**Bar to clear:** macOS title bar cleanness + Windows 11 snap affordances.
AthenaOS uses **macOS control placement** (left) on a clean bar, accent only on
focus.

### Geometry
- Titlebar height: **32px** (up from current 28). `material.mica` tint;
  focused = mica + 1px `stroke.strong` top edge; unfocused = mica darkened ~8%,
  no highlight.
- Window corners: `radius.md` (12px, from `ThemeAbi.corner_radius`); client
  background `bg.raised`. Whole window casts `elev.2` resting / `elev.4` while
  dragged.
- Title text: centered or left at `space.4`, `type.label`, `text.primary`
  (focused) / `text.tertiary` (unfocused).

### Controls (traffic-light style, left, macOS order)
- Three 14px circles, `space.2` gaps, vertically centered, `space.3` from left
  edge: **close** (`state.danger`), **minimize** (`state.warn`), **maximize**
  (`state.ok`).
- Resting: circles at ~55% saturation. Hover over the *cluster*: full
  saturation + a glyph appears inside each (×, −, +) in dark ink. Per-button
  hover: `motion.micro` brighten.
- This replaces the current right-aligned square `X/+/_` buttons in
  `window_chrome.rs`.
  - *Familiarity note:* offer a theme variant that mirrors to right-side
    Windows-style controls (Bauhaus/Windows-familiar themes) — the layout is a
    `ThemeAbi`-selectable variant, not a hardcode. Default = left/traffic-light.

### States
- **focused vs unfocused:** titlebar tint + title text token + top-edge
  highlight (above). Controls desaturate when unfocused.
- **focus ring (keyboard window switch / Alt-Tab landing):** 2px `accent.base`
  window border + `elev.focus` glow for `motion.fast`, then settles to `elev.2`.
- **maximize/restore:** `motion.emphasized` geometry tween.
- **disabled control** (e.g. non-resizable → maximize): `text.tertiary`, no
  hover.
- **reduced-motion:** instant tint swap; no maximize tween (jump-cut).
- **dark/light:** per palette.

### Snap / tiling delta
- The compositor *has* tiling but does not resize-to-fill (per checklist). Add a
  **snap-preview overlay**: dragging a titlebar to a screen edge shows a
  `material.glass` `accent.subtle` ghost of the target region (`radius.md`,
  `motion.fast` fade); release snaps + resizes-to-fill. This is the missing
  resize-to-fill made visible. (Implementer note: `tiling_wm.rs` + compositor.)

---

## 5. Notifications / toasts

**Bar to clear:** macOS notification stacking + Windows action-center grouping.
**Already strong in `notify.rs`** — this spec only tokenizes it.

- Geometry: keep 320×72, top-right, anchored `space.4` from top + right.
- Material: `material.glass` (`radius.md`, `elev.2`) — currently solid
  `CARD_BG`; move to glass tint + top-edge highlight.
- Stack: up to `MAX_VISIBLE=3`, `space.2` (8px) vertical gap, newest on top,
  oldest evicted (already implemented). Each stacked card sits 4px back in
  scale (96%) and slightly dimmer — depth cue.
- Urgency bar (left 4px): `state.*` per urgency (`BAR_NORMAL` → `accent.base`,
  critical → `state.danger`, low → `text.tertiary`). Already present; map to
  tokens.
- Content: title `type.label` `text.primary`, body `type.caption`
  `text.secondary`, source/time `type.caption` `text.tertiary`.
- Motion: in = slide from right + fade, `motion.fast`; auto-dismiss fade
  `motion.exit`; hover pauses the dismiss timer (already a timer); click =
  activate source.
- **Never steals focus** (already the contract). **Reduced-motion:** appear/
  disappear instantly, no slide.
- **dark/light:** glass tint per palette.

---

## 6. Cohesion acceptance (the whole-shell test)

Because incoherence is the top risk, the shell ships only when:
1. Taskbar, Start, quick-settings, titlebar, and a toast all read the **same
   `accent.base`** (proven by switching one Vibe Mode preset and seeing every
   surface change together).
2. Corner radii are concentric (no child sharper than its parent − padding).
3. Every interactive element has a visible focus state distinct from hover.
4. Dark and light both render with passing contrast (athena-accessibility sign-off).

---

## Handoff

### Implementers
- **athena-ui (framework):** create `athui::tokens` (spacing/radius/type/motion/
  color constants from `design-language.md`) + `derive_accent(seed)`; refactor
  `desktop.rs`, `notify.rs`, `window_chrome.rs`, and the per-app palettes to
  consume it (ends the ~30-file `const ACCENT` duplication).
- **athena-shell-apps (the surfaces):** taskbar geometry/states (`desktop.rs`);
  Start menu search + grid + motion (`start_menu.rs`/`desktop.rs`);
  quick-settings flyout (new); toast tokenization (`notify.rs`).
- **athena-gfx:** titlebar repaint with traffic-light controls + focused/unfocused
  tint + `material.mica` (`window_chrome.rs`, `compositor.rs` titlebar draw);
  snap-preview overlay glass region; confirm per-corner rounded-rect mask.
- **theme_engine (kernel):** `derive_accent`, `material.mica` wallpaper-average
  sampling, left-vs-right control-layout variant flag.
- **athena-accessibility (flagged):** contrast pass on both palettes; focus-state
  audit; reduced-motion path verification; 32px target audit.

### On-screen / boot-log evidence (athena-visual-qa + smoketests)
- **Taskbar:** QEMU screenshot showing 44px mica bar, Start pill (`radius.pill`),
  running-app accent indicators, tray clock+icons. Boot log: existing desktop
  activate marker + a new `[shell] taskbar: mica tint=0x.. accent=0x..` line.
- **Start menu:** screenshot of glass panel open over wallpaper (blur visible),
  search field focused, pinned grid 6-wide, footer. Log: `[shell] start: open
  blur=16 radius=16`.
- **Quick-settings:** screenshot of flyout with toggle tiles (on/off states
  visible) + sliders. Log: `[shell] quicksettings: N tiles`.
- **Window chrome:** screenshot of a focused vs unfocused window side by side —
  traffic-light controls, `radius.md` corners, `elev.2` shadow, focused top-edge
  highlight. Log: `[chrome] titlebar h=32 focus tint=0x.. controls=left`.
- **Toasts:** screenshot of 3 stacked glass toasts with urgency bars; the
  existing `notify::run_boot_smoketest` already prints
  `MAX_VISIBLE`/eviction — extend it to assert the urgency bar uses
  `accent.base`/`state.danger` tokens.
- **Cohesion:** screenshot before/after one Vibe Mode preset switch showing
  taskbar + Start + titlebar accent all changing together (the §6 test). Log:
  `vibe_mode::run_boot_smoketest` already prints applied accent; extend to
  assert taskbar/chrome/start all read the new `accent.base`.

### Unblocks (MasterChecklist)
- Phase 8 (AthUI/AthKit): shared token module, glass material API.
- Phase 13 (Customization): accent-derivation makes Vibe Mode coherent end-to-end.
- Phase 14 (AthShell + apps): taskbar/Start/quick-settings/chrome polish,
  notification material, snap-preview (the tiling resize-to-fill delta).
