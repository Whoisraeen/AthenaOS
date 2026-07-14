# Design Spec: GameOS Mode (PARKED — not an AthenaOS product goal)

> **Athena note:** Couch / big-picture gaming UI is **abandoned** for AthenaOS.
> See [`LEGACY_GAMING_CONCEPT.md`](../../LEGACY_GAMING_CONCEPT.md) and
> [`PARKED_GAMING.md`](../PARKED_GAMING.md). Do not expand this surface.

**Historical bootstrap text follows (do not treat as roadmap).**

# Design Spec: GameOS Mode (couch / big-picture / controller-first shell)

> *"Gaming isn't a mode. It's the default."* — legacy gaming thesis (abandoned)
>
> *"GameOS Mode — couch UI, big-picture, controller-first. Toggle into it
> instantly. Same OS, different shell."* — legacy Concept (abandoned)
>
> *"Boot directly into GameOS Mode if configured. Couch UI, controller-driven,
> optimized for living-room use. Same OS, same library, same saves as the desktop
> experience. Competes with SteamOS on its home turf with a better app ecosystem."*
> — legacy Concept (abandoned)

**Bar to clear:** Steam Big Picture / SteamOS Gamescope (the gold standard for
couch/controller UI) on responsiveness and library aggregation; Xbox dashboard on
the Game Bar / quick-resume feel; PlayStation UI on the focused-tile home;
Nintendo Switch home on instant, lag-free navigation. **GameOS must out-cohere all
four** — it is the *same OS* re-shelled, reading the *same live accent/tokens* as
the desktop, not a bolted-on launcher.

**All tokens below are defined in [`design-language.md`](./design-language.md) and
live in `components/ath_tokens`.** This spec assigns existing tokens and adds a
small, named **couch type-ramp scale-up** + **focus-ring** set (flagged for the
master doc). It introduces no other magic numbers.

---

## Already built (delta only — verify-before-spec)

The biggest single finding: **GameOS is not greenfield.** A substantial,
shell_runner-wired couch surface exists and has a passing boot smoketest. The work
is a **re-skin onto live tokens + state completeness + real controller binding**,
NOT a rebuild (CLAUDE.md rule 7 — extend the wired module, never twin it).

| Piece | Where | Today | This spec changes |
|---|---|---|---|
| Couch shell (home/library/detail/quick-menu/settings/search/carousel) | **LIVE: `components/athshell/src/gameos.rs::GameOsShell`** — full render + D-pad nav + launch | hardcoded local palette (`ACCENT 0xFF4E9CFF`, `CARD_W 200`, `GLYPH_W 8`), 8px bitmap glyphs | → read `ath_tokens` (`derive_accent` of the **live seed**, palette, spacing, radius, motion); couch type-ramp; AA text (RaeSans) not 8px block glyphs |
| Toggle in/out | **LIVE: `kernel/src/shell_runner.rs::toggle_gameos`** (F11; keyboard→controller routing; `couch: Option<GameOsShell>` in `ShellRunnerState`) | F11 + key routing only | → add controller Guide-chord + auto-on-controller-connect + auto-on-TV-out; spec the cross-fade transition |
| Boot-into-couch | **LIVE: `config_registry "/gameos/boot_couch"`** read at shell start | boolean read works | → wire to GameOS Settings → General toggle + OOBE "Game Station" profile |
| Keyboard→controller map | **LIVE: `shell_runner.rs` scancode→`GamepadButton`** | arrows/Enter/Backspace/Tab/F11 | → reference map only; the real binding is live gamepad (below) |
| Generic HID gamepad decoder | **LIVE: `kernel/src/hid_gamepad.rs`** — report-descriptor parser, axes/buttons/hat, host-KAT'd, `/proc/athena/hid_pad` | parses + decodes; **not bound to xHCI interrupt-IN yet** (iron half open) | → consumes its `PadInput`; glyph set + chord layer sit on top |
| Per-game profiles | **LIVE: `kernel/src/game_profile.rs`** (syscalls 58–61, `/proc/athena/games`, 3 presets) + **`components/athplay` `GameProfile`** (display/gpu/audio/input/sched/compat) | store + apply wired; **compositor/audio/cpufreq setters not exposed yet** (logged-intent) | → the per-game profile *surface* edits these; setters are athena-gfx/athena-kernel work, referenced not designed here |
| Library aggregation | **LIVE: `components/athplay`** — Steam(VDF)/Epic(JSON)/GOG/AthStore/Manual connectors, `GameEntry`, launch manager | parsers + state machine real; cover-art is a hash, not pixels | → couch grid renders `athplay::GameEntry`; cover-art fetch is a AthPlay/store concern |
| Game Bar overlay | partial: `GameOsState::Overlay` enum + `NowPlaying{fps,frametime}` bar exist | now-playing strip only | → full Game Bar overlay spec (FPS/frametime graph/temps/power/capture/voice), Guide-chord invoked |

> **Note for the implementer mid-build:** `gameos.rs` currently `#![allow(unused)]`
> and carries `static mut GAMEOS_ACTIVE` (the `enter_gameos_mode`/`is_gameos_mode`
> free fns) **in parallel** with the real `ShellRunnerState.couch` ownership. The
> `static mut` path is the dead twin — the live toggle is `shell_runner::toggle_gameos`.
> When you wire tokens, retire the `static mut` (rule 7), don't grow it.

---

## Concept promise → bar to clear (per surface)

| Surface | Concept line | Rival to beat | Beat it by |
|---|---|---|---|
| Entry/exit | "Toggle into it instantly. Same OS, different shell." | Steam Deck Steam/Desktop toggle (logout-feel, slow) | Instant cross-fade, **no session restart** — the compositor just swaps which shell owns the scanout |
| Couch home | "Couch UI… optimized for living-room use" | PS5 focused-tile home, Switch home | One coherent grid that reads the live Vibe accent; large focused cover-art with parallax |
| Library | "Steam, Epic, GOG, AthStore unified" (AthPlay) | Steam library (Steam-only) | True cross-store aggregation in one grid, store badge per tile |
| Game Bar | "Game Bar that doesn't suck… all native, all fast" | Xbox Game Bar (laggy, Win-tax) | Compositor-native overlay, Guide-chord, <1-frame to paint, zero OBS-class overhead |
| Per-game | "resolution, refresh rate, audio device, GPU power limit… auto-applied" | NVIDIA CP + Adrenalin + Steam Properties sprawl | One surface, one record (`game_profile`), auto-applied on launch |
| Controller | "DualSense + Xbox + every controller, full feature parity" | SteamInput glyph system | Full no-keyboard nav + multi-glyph-set button hints + OSK |

---

## Prior art distilled

- **Steam Big Picture / Gamescope (the gold standard).** Single full-screen grid;
  large focused tile with a soft scale + glow on the selected item; persistent
  bottom **button-hint bar** (A=Select, B=Back, X=context, Y=…); the **Guide/STEAM
  button** opens a side quick-access overlay *over the running game* without
  exiting it; on-screen keyboard slides from the bottom. Gamescope composites the
  game at a fixed resolution and upscales (FSR) — the *exclusive-fullscreen direct
  path* is the lesson. **Avoid:** the Desktop↔Big-Picture switch *feels like a
  logout*; controller text entry is still slow.
- **Xbox dashboard.** Quick-resume; the Guide button opens a *vertical* quick
  panel; tiles are large, sparse, motion-rich. **Avoid:** ad/promo tiles in the
  home (Concept §Windows Pain Points forbids ads — burned into the EULA); home is
  cluttered with store upsell.
- **PlayStation 5 UI.** Horizontal app rail at top, large context area below for
  the focused game (screenshots, activities); strong focused-item emphasis.
  **Avoid:** the split between "Game" and "Media" home is confusing; deep settings
  are a maze.
- **Nintendo Switch home.** Ruthlessly simple, instant, lag-free row of large
  square icons; settings/sleep on a bottom utility row. **Avoid:** too little
  density for a large PC library; no real metadata on the home.
- **SteamOS deck-UI tokens to harvest:** focus = scale-up + accent glow (not just
  a border); hint bar is *always present* and context-sensitive; the radial/quick
  menu is reachable from anywhere with one button.

---

## AthenaOS design tokens (what this surface uses)

GameOS is the desktop shell's tokens at **couch distance**. Same palette, same
accent ramp, same motion curves — scaled up for 10-foot viewing and 48px+ hit
targets. **Cohesion rule: the accent is the live seed, not a constant.**

### Color / accent (from `ath_tokens`)
- Base palette: `DARK` (couch defaults to dark — TV/OLED, evening use). Light
  parity supported via `LIGHT` for daytime/handheld.
- Accent: `derive_accent(active_seed(), &DARK)` — the **same** seed the desktop and
  Vibe Mode use. Selected tiles, focus ring, progress, hint glyphs all key off
  this ramp (`accent.base` / `accent.hover` / `accent.active` / `accent.subtle` /
  `accent.glow`). Replaces the file-local `const ACCENT 0xFF4E9CFF`.
- Surfaces: `bg.base` (void behind grid), `bg.raised` (cards/tiles resting),
  `bg.elevated` (hovered/selected row fill), `bg.overlay` (quick-menu / Game Bar
  glass fallback), `stroke.subtle` (tile hairline), `stroke.strong` (glass
  top-edge). State colors: `state.ok` (installed/FPS≥target), `state.warn`
  (FPS mid / updating), `state.danger` (FPS low / error).

### Spacing & grid (4px grid, `ath_tokens` §2)
- Inter-tile gap: **`space.5` (24px)** at 1080p, **`space.6` (32px)** at ≥1440p
  (replaces the local `CARD_GAP 16` — too tight for couch).
- Grid outer margin: **`space.8` (48px)** (the "large couch-mode gap" token).
- Section gap (Featured ↔ Recently Played ↔ Library): **`space.8` (48px)**.
- Hint-bar / quick-menu inner padding: **`space.4` (16px)**.

### Hit targets (`ath_tokens`)
- **`HIT_TARGET_COUCH = 48px` is the floor** for every focusable element (tiles,
  hint chips, OSK keys, quick-menu rows). The desktop's 32px floor does NOT apply
  in couch mode — controller + TV distance demand 48px.

### Corner radius (`ath_tokens` §3)
- Game tiles / cover-art cards: **`radius.lg` (16px)**.
- Quick-menu / Game Bar / OSK panels: **`radius.lg` (16px)**; nested rows use
  `concentric(radius.lg, space.4)` = `radius.xs` so corners never mismatch.
- Hint-bar chips / OSK keys: **`radius.md` (12px)**.

### Elevation (`ath_tokens` §5.3)
- Resting tile: `elev.1`. Selected/focused tile: `elev.3` + **focus glow**
  (`elev_focus(accent.glow)`) — the SteamOS lesson: focus is *lift + glow*, not
  just a 1px border.
- Quick-menu / Game Bar / OSK: `elev.3` (modal class).
- Game-detail panel: `elev.4`.

### Motion (`ath_tokens` §7)
- Focus move tile→tile: **`motion.micro` (90ms, standard-out)** — must feel
  instant (Switch-class). Focus ring + tile scale animate on this curve.
- Page/section change: **`motion.standard` (220ms, decelerate)**.
- Quick-menu / Game Bar slide-in: **`motion.fast` (140ms)** in, **`motion.exit`
  (120ms)** out.
- Desktop↔GameOS cross-fade: **`motion.emphasized` (320ms)** — same curve as Vibe
  Mode transition (it *is* a personality switch).
- **Reduced-motion:** all collapse to `MOTION_INSTANT` (0ms); focus then snaps
  (ring still moves, just no tween) — focus must never become invisible (§a11y).

### Couch type ramp — **NEW token group (flag to master doc)**
The desktop `type.*` ramp is sized for ~50cm. Couch viewing is ~3m (≈6×). Rather
than a free scale, define a fixed couch ramp = desktop ramp × ~1.6–2.0, rounded to
the 4px grid, capped so a 1080p TV still fits a useful grid. **All weights/line-
heights inherit the desktop ramp's intent (600 for titles, 500 subtitle, 400
body).**

| Couch token | px | weight | line-height | Desktop origin | Use |
|---|---|---|---|---|---|
| `type.couch.hero` | 48 | 600 | 56 | display(32) ×1.5 | Focused-tile title, big clock |
| `type.couch.title` | 36 | 600 | 44 | title(22) ×1.6 | Section headers ("Featured") |
| `type.couch.subtitle` | 28 | 500 | 36 | subtitle(17) ×1.6 | Tile titles, quick-menu labels |
| `type.couch.body` | 22 | 400 | 30 | body(14) ×1.6 | Metadata, settings values |
| `type.couch.label` | 20 | 500 | 26 | label(13) ×1.5 | Hint-bar text, store badges |
| `type.couch.caption` | 17 | 400 | 22 | caption(11) ×1.5 | Timestamps, secondary hints |

Minimum on-screen text in couch mode is **`type.couch.caption` (17px)** — nothing
smaller (the 8px bitmap glyph in `gameos.rs` today fails the 10-foot bar outright).

### Focus ring — **NEW spec (flag to master doc)**
Couch focus is the single most important visual; it must be unmistakable across the
room and color-blind-safe (never *color alone*).

- **Ring:** 4px stroke in `accent.base`, inset by `space.1` (4px) from the tile
  edge, at the tile's `radius.lg`. Plus the `elev_focus(accent.glow)` glow (radius
  10) so it reads as a *lit* element, not an outline.
- **Tile scale:** focused tile scales to **1.06×** over `motion.micro`; neighbors
  unaffected (no Dock-magnification cascade — that's a pointer idiom).
- **Non-color cue (a11y):** focused tile also gains a top-edge `stroke.strong`
  highlight + the scale, so focus survives a 4.5:1-failing accent or a color-blind
  viewer. **Focus is lift + scale + ring + glow**, four redundant signals.
- **Contrast:** `accent.base` ring on `bg.base` must clear **3:1** (WCAG non-text
  AA); gate it with `contrast_ratio()`. If a Vibe seed fails, fall back the ring to
  `text.primary` (mirrors `accent.text`'s WCAG fallback).

---

## Surface specs

### 1. Entry / exit — "toggle into it instantly"

**The model (cohesion-critical):** GameOS is a *shell swap on one running session*,
not a logout. `ShellRunnerState.couch: Option<GameOsShell>` already encodes this —
the desktop shell and couch shell are two render front-ends over the same
compositor, processes, library, and saves. Entering GameOS does **not** restart the
session (beating Steam Deck's logout-feel toggle).

**Triggers (all route to `shell_runner::toggle_gameos`):**
1. **Hotkey:** F11 (LIVE) — keep. Add a rebindable chord in desktop Settings.
2. **Controller chord:** **Guide held 1s** from the desktop → enter GameOS (the
   "press the big button to go to the couch" idiom). Guide *inside* GameOS opens
   the quick-menu (LIVE); Guide-held *exits* to desktop.
3. **Auto on controller-connect:** if `/gameos/auto_on_pad` is set and the active
   input becomes a gamepad (HID gamepad bound on xHCI), prompt-then-enter (toast
   with a 5s "Enter GameOS? [A]" — never yank the screen unprompted).
4. **Auto on TV-out:** on a new HDMI display marked as a TV (EDID CEA detailed
   block / large diagonal), offer GameOS via the same toast.
5. **Boot-into:** `/gameos/boot_couch` (LIVE) — Game Station profile / OOBE choice.

**Transition motion:** cross-fade `motion.emphasized` (320ms). Desktop fades to a
brief `bg.base` wash, GameOS home fades up with the focused tile already scaled in.
The wallpaper persists underneath (same Vibe wallpaper) so it reads as one
environment shifting, not two apps swapping. Reduced-motion → instant cut.

**Exit** mirrors entry: Guide-held / F11 / quick-menu "Desktop Mode" (LIVE
`QuickAction::DesktopMode`). A game running in GameOS keeps running and reappears on
the desktop (same process) — no relaunch.

### 2. The couch home (10-foot UI)

Layout, left→right / top→bottom, all on the 4px grid with `space.8` outer margin:

- **Top status rail** (`type.couch.label`): user badge (avatar + name + online
  dot in `state.ok`), big clock (`type.couch.hero` time, `type.couch.caption`
  date), and a compact status cluster (network/battery/volume glyphs at 48px hit
  targets). Replaces the cramped 220px sidebar badge for TV legibility.
- **Left nav rail** (the LIVE sidebar, widened): Home / Library / Recent /
  Favorites / Store / Friends / Downloads / Screenshots / Settings. Each row ≥48px,
  `type.couch.subtitle`, selected row = `bg.elevated` + left `accent.base` bar +
  focus ring. Collapses to icon-only when focus is in the grid (PS5 rail idiom).
- **Featured carousel** (LIVE `render_carousel`): one large focused cover-art
  (`radius.lg`, `type.couch.hero` title overlaid on a bottom gradient scrim),
  flanked by partially-visible neighbors; auto-advances on `carousel_speed_ms`
  (pause on focus). Dot indicators in `accent.base`/`text.tertiary`.
- **Recently played + Library grid** (LIVE `render_game_grid`): cover-art tiles,
  `space.5`/`space.6` gaps, store badge chip (STM/EPC/GOG/RAE) in the corner,
  `state.ok` "Ready" / `text.tertiary` "Not installed", favorite star in
  `state.warn`/gold. Columns auto-fit to width (LIVE `grid_columns`), re-derived
  for the couch tile size (larger than the current 200×140 — target ~`3:4`
  cover-art at ≥260px wide so cover art is legible at 3m).
- **Persistent button-hint bar** (bottom, **NEW** — the SteamOS staple): always
  shows the context-sensitive controller actions as glyph+label chips
  (e.g. `(A) Play  (X) Details  (Y) Search  (☰) Menu  (B) Back`). `type.couch.label`,
  glyphs from the active glyph set (§5). This is the single biggest legibility win
  over the current bracketed-text hints (`[A] Play`).

**Library aggregation:** the grid is fed `athplay::GameEntry` from all enabled
connectors (Steam/Epic/GOG/AthStore/Manual — all LIVE parsers). One unified library,
store badge per tile. AthPlay owns scan/launch; GameOS owns the *presentation*.

**Focus navigation model:** D-pad / left-stick moves focus one tile per press
(`motion.micro`); LB/RB page; LT/RT jump section; left-edge → nav rail; up from grid
→ status rail (all LIVE in `handle_button`). The focus ring + 1.06× scale (§focus
ring) is the visible state.

### 3. In-game overlay / Game Bar — "doesn't suck, all native, all fast"

Invoked by **Guide (tap)** while a game runs (Guide-held = exit to desktop). A
compositor-native overlay (`material.glass`, `bg.overlay` tint, `elev.3`,
`radius.lg`) slides from the right (`motion.fast`) over the running game — the game
keeps rendering on the direct path; the overlay composites above it. **No OBS, no
Win-tax** (the Xbox Game Bar lesson).

Panels (vertical, controller-navigable, each row ≥48px):
- **Performance:** live FPS (large, `type.couch.hero`, colored `state.ok`/`warn`/
  `danger` vs the profile's target refresh), a **frametime graph** (last ~2s,
  scrolling, accent line + 16.6/8.3ms reference rules), CPU temp + GPU temp
  (from `thermal`/`amd-smn` — LIVE k10temp path), CPU/GPU utilization.
- **Power profile:** the active per-game profile name + a quick toggle between
  presets (Competitive/Balanced/Cinematic — LIVE `game_profile` presets) and
  NULL_LATENCY (LIVE `FLAG_NULL_LATENCY`).
- **Capture:** screenshot (LIVE `QuickAction::Screenshot` / `screenshot-capture.md`)
  and record toggle (compositor capture — "zero-cost recording" per Concept).
- **Voice / audio:** voice-chat device + push-to-talk indicator, game/voice volume
  mix (the "VoiceMeeter-class native audio routing" promise).
- **Quick system:** brightness, volume, Wi-Fi, Do-Not-Disturb, friends, downloads,
  sleep, desktop-mode (LIVE `QuickMenu`).

Data source: this consumes `/proc/athena/perf` (the missing telemetry surface per
CLAUDE.md North-Star table) + `NowPlaying{fps,frametime}` (LIVE). The frametime
graph is the one net-new draw primitive.

### 4. Per-game profile surface

Edits the existing record — **does not invent a new schema.** Backed by
`kernel/src/game_profile.rs` (`GameProfileAbi`, syscalls 58–61) and the richer
`athplay::GameProfile` (display/gpu/audio/input/scheduler/compat). Reached from a
tile's detail view ("⚙ Profile") or Settings → Games.

Fields (grouped, controller-editable rows, `type.couch.body` values):
- **Display:** resolution, refresh rate, HDR mode (`HdrMode`), VRR on/off, vsync,
  fullscreen mode (`FullscreenMode::ExclusiveFullscreen` = the direct path),
  scaling filter (incl. FSR).
- **GPU:** power-limit %, max frame-rate cap, shader-cache toggle, fan curve,
  force-low-latency.
- **Audio:** output device, spatial mode (`SpatialAudioMode` incl. HRTF), voice
  device + ducking.
- **Input:** controller layout (Default/Xbox/DualSense/SteamDeck/Custom), gyro,
  adaptive triggers, vibration, mouse sens.
- **Scheduler:** SCHED_BODY on, NULL_LATENCY, background throttle, core affinity,
  render-thread pinning.

**Auto-apply:** on launch, AthPlay's `LaunchManager` applies the profile via
`LaunchCallbacks` (LIVE trait) → `game_profile::apply_profile` → scheduler (LIVE) +
compositor/audio/cpufreq setters (**not yet exposed** — athena-gfx/athena-kernel).
The surface writes the record; the kernel applies it. Snapshot/rollback rides on
`config_registry` (LIVE) so a bad tweak is one click back.

### 5. Controller-first interaction model

**Full navigation with no keyboard/mouse** is the contract. The LIVE
`handle_button` already implements D-pad focus, A=select, B=back, X=launch,
Y=search, Guide=quick-menu across all focus targets. This spec adds the missing
input + presentation layers:

- **Live gamepad binding:** consume `hid_gamepad::PadInput` (LIVE decoder) — bind
  it to xHCI interrupt-IN endpoints (the iron half, athena-gaming/athena-kernel).
  First-party pads (DualSense/Xbox, decoded by VID/PID in `input.rs`) get the
  deluxe path (haptics/triggers/gyro per Concept); everything else gets correct
  generic HID (the "never nothing" rule in `hid_gamepad.rs`).
- **Button-glyph system (NEW):** a glyph set abstraction with three skins —
  **Xbox** (A/B/X/Y colored), **PlayStation** (✕/◯/△/▢), **Steam/generic**
  (lettered). Auto-selected from the bound pad's VID/PID; user-overridable
  (`athplay::ControllerLayout`). Every hint chip + OSK + quick-menu renders the
  active set. This replaces the hardcoded `[A]`/`[B]` ASCII in `render_*` today.
- **On-screen keyboard (NEW):** slides from the bottom (`motion.fast`), QWERTY +
  symbol layers, focus-navigated by D-pad, A=type, LB/RB=layer, LT/RT=space/back;
  predictive row at top. Keys ≥48px (`HIT_TARGET_COUCH`), `radius.md`,
  `type.couch.subtitle`. Used by search and any text field. (Steam-class, but the
  predictive row + always-on-screen target it past Steam's slow grid.)
- **Chord layer:** Guide-tap = Game Bar / quick-menu; Guide-hold(1s) = exit;
  Guide+RB = screenshot; Guide+LB = record (mirrors Xbox/Steam muscle memory).

### 6. Cohesion + the OS gaming paths (reference, not designed here)

GameOS is the *visible face* of paths the OS already promises; it must read them,
not reimplement them:
- **Live accent/tokens:** every color/spacing/radius/motion comes from `ath_tokens`
  with `derive_accent(active_seed())`. A Vibe Mode change re-skins GameOS in lockstep
  with the desktop — *that's* the cohesion deliverable. (Today `gameos.rs` is the
  single biggest token-drift offender: ~20 file-local constants. Fixing it is the
  cohesion work.)
- **SCHED_BODY** (LIVE EDF) — the running game's threads; GameOS's own render thread
  should ride SCHED_BODY too (compositor-class), so the UI stays smooth under load.
- **Exclusive-fullscreen direct-to-GPU** (Concept §Performance) — when a game runs,
  GameOS yields the scanout to the direct path (`FullscreenMode::ExclusiveFullscreen`);
  the Game Bar composites *above* without forcing the game off the direct path.
  (athena-gfx owns the scanout handoff; `scanout-backend-seam` memory is the seam.)
- **Compositor VRR/HDR** (LIVE drop-shadow/blur/HDR tone-map in `recomposite`) —
  GameOS surfaces are glass/elevation over the same compositor; per-game VRR/HDR is
  applied by the profile, not by GameOS.

---

## States & interaction matrix

| Element | default | focus (controller) | active/press | disabled | dark | light | reduced-motion |
|---|---|---|---|---|---|---|---|
| Game tile | `bg.raised`, `elev.1` | ring + 1.06× + glow + top `stroke.strong` | tile dips to `accent.active` wash | dimmed cover + `text.tertiary` "Not installed" | default | `LIGHT` palette | scale/tween off; ring still moves |
| Nav-rail row | transparent | `bg.elevated` + `accent.base` bar + ring | — | hidden if section empty | default | light | snap |
| Hint chip | `bg.overlay`@subtle | n/a (non-focusable) | — | greyed when action unavailable | default | light | — |
| OSK key | `bg.raised` | ring + scale | `accent.active` | — | default | light | snap |
| Quick-menu / Game Bar row | `bg.overlay` glass | `bg.elevated` + `accent.base` bar | toggles | greyed | default | light | slide→instant |

**Keyboard + controller nav map (LIVE in `handle_button`, presented here as the
contract):**

| Input | Home/Grid | Detail | Game Bar/Quick-menu | OSK |
|---|---|---|---|---|
| D-pad/L-stick | move focus | — | move row | move key |
| A / ✕ | select/launch | Play/Install | activate | type |
| B / ◯ | rail / up-level | close | close | close |
| X / ▢ | details/context | — | — | backspace |
| Y / △ | search (opens OSK) | favorite | — | layer |
| LB/RB | page | — | — | layer/space |
| LT/RT | section jump | — | — | space/back |
| Guide (tap) | quick-menu | quick-menu | toggle | — |
| Guide (hold 1s) | exit to desktop | exit | exit | — |

---

## Accessibility (in scope from the start — flag to athena-accessibility)

- **Focus visibility:** the four-redundant-signal focus (ring + scale + glow + top
  highlight) is the core a11y guarantee — focus must never depend on color alone
  and must read at 3m. athena-accessibility should screenshot-verify focus on every
  surface, including with a deuteranopia simulation.
- **Contrast:** `type.couch.*` text on tiles/scrim must clear **4.5:1**; the
  bottom-of-tile gradient scrim exists specifically so title text over bright cover
  art still passes — gate with `contrast_ratio()`. Focus ring clears 3:1 or falls
  back to `text.primary`.
- **Reduced-motion:** all tweens → `MOTION_INSTANT`; carousel auto-advance pauses;
  parallax off. Focus still visibly relocates (snap).
- **Hit targets:** `HIT_TARGET_COUCH = 48px` floor, no exceptions in couch mode.
- **Text-scale:** the couch ramp should honor a global text-scale multiplier (a
  athena-accessibility setting) — a low-vision user can push `type.couch.*` further.
- **Audio cues:** Vibe Mode sound set provides focus-move / select / back sounds
  (the "sound design" half of a coherent personality) — optional, off by default.

---

## Handoff

**Implementer split:**
- **athena-shell-apps** — owns the couch-UI *surface*: re-skin `athshell/src/gameos.rs`
  onto `ath_tokens` (retire the file-local palette + the `static mut GAMEOS_ACTIVE`
  twin), the couch type-ramp, AA text (RaeSans, not 8px glyphs), focus-ring/scale,
  the button-hint bar, the OSK, the Game Bar overlay layout, the per-game profile
  *editor* UI. Plus the cross-fade transition in `shell_runner::toggle_gameos`.
- **athena-gaming** — owns the controller stack (bind `hid_gamepad::PadInput` to live
  xHCI interrupt-IN; first-party DualSense/Xbox haptics/triggers/gyro; the glyph-set
  auto-select), the per-game profile *application* wiring (`athplay::LaunchCallbacks`
  → `game_profile::apply_profile`), library aggregation (AthPlay connectors → grid),
  and the auto-enter triggers (controller-connect / TV-out).
- **athena-kernel / athena-gfx** *(referenced, not specified here)* — SCHED_BODY for
  the couch render thread, the exclusive-fullscreen direct-scanout handoff, the
  compositor/audio/cpufreq profile **setters** that `game_profile::apply_profile`
  logs intent for today, and the `/proc/athena/perf` telemetry the Game Bar reads.
- **ath_tokens (Opus / design)** — add the `type.couch.*` ramp + focus-ring tokens
  to `design-language.md` and the crate.

**Tokens added/changed (→ master doc):**
- NEW: `type.couch.{hero,title,subtitle,body,caption,label}` (couch ramp).
- NEW: focus-ring spec (4px `accent.base`, `space.1` inset, `radius.lg`,
  `elev_focus(accent.glow)`, 1.06× scale, top `stroke.strong` cue, 3:1 fallback).
- CHANGED (cohesion): `gameos.rs` must consume `ath_tokens` instead of ~20
  file-local constants; couch gaps move to `space.5`/`space.6`/`space.8`; hit
  targets to `HIT_TARGET_COUCH`.

**Visual-QA checklist (athena-visual-qa screenshots to verify):**
1. Couch home renders the aggregated grid; tiles use cover-art proportions, AA text
   (not block glyphs), store badges, `space.5/6` gaps.
2. Focus ring is unmistakable at 1:1 and at a downscaled "across-the-room" zoom;
   focused tile is scaled + glowing; works under a color-blind simulation.
3. Accent matches the live desktop accent after a Vibe Mode change (re-skin
   cohesion) — screenshot desktop + GameOS side-by-side, accents identical.
4. Button-hint bar shows the correct glyph set for the bound pad (Xbox vs PS vs
   generic).
5. Game Bar overlay composites over a running game without breaking the game's
   render; frametime graph + temps populate.
6. OSK is legible, 48px keys, focus-navigable.
7. Light-mode parity; reduced-motion (focus snaps, no carousel auto-advance).
8. Per-game profile editor shows real fields and the values round-trip
   (`/proc/athena/games`).

**Unblocks checklist lines:** Phase 14.3 (Toggle in/out of GameOS Mode — extends the
LIVE toggle), Phase 12.2 (controller stack — binds the LIVE HID decoder), Phase
13.x (per-game profile surface), and the Concept §Game Station experience.

---

## Phased build order (minimal FAIL-able slices first)

The existing smoketest is already `[shell_runner] couch smoketest: dpad_nav=… launch=…
rendered=… boot_flag_default_desktop=… -> PASS`. Each phase extends a FAIL-able proof.

1. **Token cohesion (first, smallest, highest fan-out).** Re-skin `gameos.rs` onto
   `ath_tokens` + the couch ramp + AA text + focus ring. The existing render
   smoketest still passes; add accent-cohesion + token assertions.
   **Proof:** `[gameos] couch smoketest: tiles=N focus_nav=ok accent_matches_seed=ok glyphs=aa hit48=ok -> PASS`
   (FAIL if the rendered accent ≠ `derive_accent(active_seed()).base`, or any focus
   target's hit box < 48px, or text path is the 8px block font).
2. **Button-hint bar + glyph sets.** Render the context hint chips with a selectable
   glyph set. **Proof:** `[gameos] glyph smoketest: set=xbox|ps|generic chips=N context_ok -> PASS`.
3. **Live controller bind.** `hid_gamepad::PadInput` → couch nav (replace the
   keyboard-only routing). **Proof:** decode a known pad report → focus moves the
   expected direction. (host-KAT'able off the live decoder.)
4. **Game Bar overlay.** Overlay layout + frametime graph + temps from
   `/proc/athena/perf`. **Proof:** `[gameos] gamebar smoketest: panels=N fps=… frametime_pts=… temps=ok -> PASS`.
5. **Per-game profile editor + auto-apply round-trip.** Edit → `game_profile` →
   `/proc/athena/games` reflects it. **Proof:** profile set/get/apply round-trips.
6. **OSK + auto-enter triggers + cross-fade transition.** **Proof:** OSK types into
   search; toggle cross-fade present; auto-enter toast fires on simulated
   pad-connect.

**The minimal first implementable slice is Phase 1** — it is pure presentation +
token plumbing over an already-working, already-smoketested surface, has the
highest cohesion fan-out (kills the worst token-drift offender in the tree), and
keeps the existing PASS line green while adding the cohesion assertion. Proof line:

```
[gameos] couch smoketest: tiles=N focus_nav=ok accent_matches_seed=ok glyphs=aa hit48=ok -> PASS
```
