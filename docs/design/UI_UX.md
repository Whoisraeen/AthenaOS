# RaeenOS — UI/UX Master Spec ("The Perfect Mashup")

> *"Built for people who care about how things feel."* — `RaeenOS_Concept.md`
>
> This is the **whole-OS layout + experience bible** for `raeen-ui` and `raeen-shell-apps`.
> It is the north star: the single document that describes every surface of the OS and
> the *why* behind it. It does **not** restate token-level detail — that lives in the
> per-surface specs it indexes. When this doc and a per-surface spec disagree on a
> number, the per-surface spec wins; when either disagrees with `RaeenOS_Concept.md`,
> the Concept doc wins.

## How to use this doc

| You are building… | Read this doc for | Then read |
|---|---|---|
| Any surface | §1 thesis, §2 research, §5 interaction model | the per-surface spec below |
| Tokens (color/space/type/motion) | — | `docs/design/design-language.md` (authoritative) |
| Taskbar/Start/tray/chrome/notifs | §4.2–4.9 intent | `docs/design/desktop-shell.md` (authoritative) |
| Settings | §4.10 intent | `docs/design/settings.md` (authoritative) |
| Text rendering | §3 | `docs/design/typography-rendering.md` (authoritative) |

**Index of per-surface specs (build these out; this doc is the umbrella):**
`design-language.md` ✅ · `desktop-shell.md` ✅ · `settings.md` ✅ · `typography-rendering.md` ✅ ·
**TODO specs:** `global-search.md`, `window-management.md`, `workspaces.md`, `control-center.md`,
`file-manager.md`, `gameos.md`, `vibe-mode.md`, `oobe.md`, `multi-monitor.md`, `accessibility.md`.

---

## 1. The thesis — the perfect mashup

> Windows got bloated chasing enterprise. macOS locked itself behind taste-as-control.
> Linux never solved coherence. **RaeenOS takes the best of all three and removes what
> everyone hates.**

| We take from | The thing | Why |
|---|---|---|
| **Windows** | Familiar bottom taskbar, Snap Layouts + custom zones, drag-and-drop everywhere, a real Start, broad app reach | The 1.4B-user muscle memory — switchers feel at home in 10 minutes |
| **macOS** | Spotlight-speed search, Mission Control overview, Dock polish, traffic-light window controls, *coherence* and motion taste | The "it just feels premium" bar |
| **Linux** | Total customizability, swappable WM (tile/stack/float) + swappable shell, virtual desktops, keyboard-driven power, no ads/telemetry | The "the user owns the machine" soul |

**The RaeenOS soul on top of the mashup:** Liquid-Glass-grade visuals (§3), one coherent
widget system (no Linux inconsistency), Vibe-Mode personalities, GameOS Mode, and
capability-sandboxed-by-default security — all with **zero** ads, telemetry, or forced
updates.

**The 10-minute promise:** a switcher from Windows *or* macOS is productive in 10 minutes.
We ship three **Familiarity Presets** (§6) — *Windows-like*, *macOS-like*, *RaeenOS Native* —
chosen in OOBE, each a coherent set of taskbar position, window-control side, and hotkeys.

---

## 2. Research foundation — what people hate, and how we fix it

Distilled from current (2025–26) community complaints. **Every row is a requirement.**

### 2.1 Windows 11 — the most-hated, fixed

| What people hate | RaeenOS does |
|---|---|
| Start menu is huge, clumsy, not customizable | Compact, instant, **fully customizable** launcher (resize, columns, hide recommendations, all-apps default). §4.3 |
| Taskbar locked to bottom; can't move it | Taskbar movable to **bottom/top/left/right**, per-monitor. §4.2 |
| Can't drag-drop to pin; can't drop files onto taskbar icons | Drag-drop to pin **and** drop-a-file-onto-an-app-icon-to-open, both supported. §4.2 |
| Start forced to center, no choice | Start position **configurable** (left or center). §4.2 |
| Secondary monitors lose the clock / get a degraded taskbar | **Full taskbar incl. clock on every monitor.** §4.16 |
| Ungroup / never-combine removed | Grouping is a **user choice** (combine / never-combine / combine-when-full). §4.2 |
| Ads in Start, Explorer, lock screen | **Forbidden by design** (Concept §"what we delete"). §7 |
| Settings vs Control Panel split | **One** unified, fully searchable Settings. §4.10 |
| File Explorer feels like 2007 | Modern file manager: tabs, split panes, fuzzy search, batch rename. §4.11 |
| Forced updates / reboots / telemetry / Bing-in-search / Copilot bloat | No forced updates (atomic + one-click rollback), no telemetry, **local** search, no bundled assistant. §7 |
| Right-click menu hides actions behind "Show more options" | One flat, fast, complete context menu. §5 |

### 2.2 macOS — the premium feel, minus the cage

| What people hate | RaeenOS does |
|---|---|
| "Worst window management"; native only vertical splits; needs Rectangle/Magnet | **First-class tiling built in:** keyboard snap to halves/thirds/quarters, Snap Layouts, custom zones. §4.5 |
| Alt/Cmd-Tab shows one window per app, not all | App-switcher cycles **every window**; per-app sub-cycle too. §4.5 |
| Menu-bar icon clutter needs Hidden Bar | Tray/menu icons are **user-manageable** (show/hide/overflow). §4.9 |
| Not customizable like Linux | Tweakable everything; swappable WM + shell. §4.5, §7 |
| Finder gaps (cut/paste history, path bar, batch) | File manager has cut/paste, path bar, batch ops, dual-pane. §4.11 |
| Maximize ("green button") is unpredictable | Predictable maximize **and** snap-to-fill; hold for layout picker. §4.5 |

### 2.3 Linux desktop — the freedom, minus the chaos

| What people hate | RaeenOS does |
|---|---|
| Fragmentation — a dozen DEs, none cohesive | **One** first-class shell (RaeShell), one design language — *swappable* but never fragmented. §1, §7 |
| Inconsistency — scrollbars/widgets differ per app | **One widget system** (RaeUI). Every app uses the same tokens → total visual consistency. §3 |
| HiDPI / fractional scaling is broken | Proper **fractional scaling + per-monitor scale** from day one. §4.16 |
| App availability | RaeBridge (Windows-app compat) + RaeStore + PWA. (Concept) |
| Theming breaks apps / half-themed | Theming is a **first-class signed bundle** (Vibe Mode) the whole stack honors. §4.15 |

### 2.4 Features users actively want (our answer)

| Want | RaeenOS |
|---|---|
| App store: clean install/uninstall, trials, auto-update, gradated trust | **RaeStore** (.raepkg, Wasm sandbox tiers) |
| Sandbox mode that makes malware infection impossible | **RaeShield** capabilities — sandbox-by-default, per-app manifests |
| Central 3rd-party update management | RaeStore unified updates; atomic + rollback |
| Remote control through NAT/firewalls | RaeSync / remote (Concept) |
| A real command palette / keyboard-driven everything | **Global Search + Command Palette** (§4.4) — the headline power feature |

### 2.5 What power users LOVE — we keep and out-execute

Spotlight instant search · Mission Control overview · the Dock · Snap Layouts + Snap Groups +
FancyZones custom zones · auto-tiling (Pop/COSMIC) · virtual desktops/Spaces · i3/yabai
keyboard tiling · PowerToys-class utilities. **RaeenOS ships these as defaults, not
third-party bolt-ons.**

---

## 3. Visual language at a glance

Token-level detail is **authoritative in `design-language.md`** — do not re-invent. The
direction:

- **Liquid-Glass-grade depth, used with discipline.** Glass/blur creates *hierarchy and
  focus*, not decoration (the "beautiful trap" — readability and contrast must always win).
  `material.glass` for transient/floating surfaces (menus, flyouts, command palette,
  notifications); `material.mica` for large always-on surfaces (taskbar, sidebars). Soft
  edge-fade, light-aware layering, real source-over compositing against the actual backdrop.
- **Typography is the make-or-break.** RaeSans (Inter) for all chrome/body, RaeMono
  (JetBrains Mono) for code/terminal, **grayscale anti-aliased** via `Canvas::draw_text_aa`
  — the 8×8 bitmap block font is BANNED from any user-facing surface (it is the single
  biggest "looks basic/retro" signal). Honor the §6 type ramp. See `typography-rendering.md`.
- **Motion with intent.** Spring/ease curves from the §7 motion system, at the display
  refresh rate; every animation respects **reduced-motion**.
- **One accent, system-wide.** The accent color (Vibe-Mode-driven) is the cohesion engine —
  selection, focus rings, primary buttons, active states all derive from it.
- **Dark default, first-class light.** Both palettes are real, not an afterthought.
- **Accessibility is a token, not a mode** — contrast ≥ WCAG AA, focus rings on everything,
  hit targets ≥ spec, high-contrast + reduced-motion built in. (§4.17)

---

## 4. The whole-OS layout map

Every surface. Each entry: **purpose → layout → the mashup decision → status**.

### 4.1 Desktop
- **Purpose:** wallpaper canvas, optional icons, optional live widgets.
- **Layout:** full-bleed wallpaper (static or live, paused when occluded); right-click
  desktop = quick context (new folder, display, personalize, Vibe Mode); optional
  Rainmeter-class widgets (clock, system, media). Icons optional and off by default
  (macOS-clean) but available (Windows-familiar).
- **Mashup:** macOS clean canvas + Windows icon affordance + Linux live-widget freedom.

### 4.2 Taskbar / Dock — `desktop-shell.md §1` (authoritative)
- **Purpose:** the persistent anchor — running apps, pins, tray, clock, Start.
- **Layout (default):** bottom bar, `material.mica`. Left/center Start (configurable),
  pinned + running apps (grouped per user choice, live previews on hover), right tray
  cluster + clock + control-center button.
- **Mashup decisions (all the W11 fixes from §2.1):** movable to any edge, per-monitor with
  clock, drag-drop pin + drop-file-on-icon, configurable Start position, optional Dock-style
  **magnification** for the macOS crowd.
- **Status:** built (delta polish in `desktop-shell.md`).

### 4.3 Start / Launcher — `desktop-shell.md §2`
- **Purpose:** find and launch apps; the home base.
- **Layout:** compact panel (NOT the W11 full-screen sprawl): search field at top (feeds the
  global search, §4.4), pinned app grid, recents/recommendations (toggleable off), All Apps
  list, power/user controls at the bottom.
- **Mashup:** Windows familiarity + macOS Launchpad density + the speed of Spotlight + Linux
  "turn off the noise."

### 4.4 Global Search + Command Palette — **TODO `global-search.md`** ⭐ headline feature
- **Purpose:** one keystroke to *anything* — apps, files, settings, web, math, unit convert,
  and **actions/commands** (e.g. "toggle dark mode", "connect Bluetooth", "new workspace").
- **Layout:** centered floating glass palette (Super/Cmd+Space). Instant fuzzy results,
  grouped (Apps / Files / Settings / Actions / Web), keyboard-first, type-to-run.
- **Mashup:** Spotlight speed + macOS/raycast command-palette power + Windows search breadth —
  but **100% local** (no Bing, no ads, no telemetry). This is RaeenOS's signature.
- **Status:** gap — top WS4 priority.

### 4.5 Window management — **TODO `window-management.md`**
- **Purpose:** the multitasking core — RaeenOS's answer to *the* most-hated thing on both
  macOS and Windows.
- **Modes (swappable per the Concept):** **float** (default, macOS-style), **tile** (i3-style
  auto-tiling), **stack**, **hybrid** — a first-class API, switchable live.
- **Snapping (built in, no third-party):** drag-to-edge snap with fill suggestions
  (Pop/Ubuntu Tiling Assistant); **Snap Layouts** picker on the maximize button + on a
  hotkey (Windows); keyboard snap to halves/thirds/quarters/corners (Rectangle); **custom
  zones** (FancyZones); per-monitor.
- **Switcher:** Alt/Super+Tab cycles **all windows** (fixes the macOS one-per-app gripe);
  Super+\` sub-cycles within an app; an Exposé/overview grid (§4.6).
- **Chrome:** traffic-light controls, macOS order, left (`desktop-shell.md §4`); predictable
  maximize; drag title to move, double-click to maximize, drag-to-edge to snap.
- **Status:** snapping/tiling partially built (`wm_policy.rs` computes layout but lacks the
  resize-to-fill protocol — a real gap); switcher exists (Alt-Tab).

### 4.6 Workspaces / virtual desktops + Overview — **TODO `workspaces.md`**
- **Purpose:** organize work across multiple virtual screens; bird's-eye overview.
- **Layout:** N virtual desktops, per-monitor, named; an **Overview** (Mission-Control /
  GNOME-Activities) showing all windows + workspaces in a zoomable grid; gesture
  (3-finger/edge) + keyboard (Super+number, Super+arrows) navigation.
- **Mashup:** macOS Spaces + GNOME Activities + i3 workspaces.
- **Status:** gap.

### 4.7 Control Center / Quick Settings — `desktop-shell.md §3`
- **Purpose:** one panel for the toggles people poke constantly.
- **Layout:** glass flyout from the tray: Wi-Fi, Bluetooth, volume, brightness, Vibe Mode,
  Do-Not-Disturb, night light, airplane, screen record, battery/perf profile; sliders +
  toggle tiles; expandable rows (click Wi-Fi → pick network inline).
- **Mashup:** macOS Control Center polish + Windows quick-settings practicality. One panel,
  not the W11 split between Quick Settings and the notification flyout.

### 4.8 Notifications / Notification Center — `desktop-shell.md §5`
- **Purpose:** transient toasts + a persistent history.
- **Layout:** toasts top-right (glass, actionable, auto-stack, swipe-to-dismiss); a
  Notification Center (history, grouped by app, with inline actions); **Do Not Disturb** +
  Focus modes. No notification = no nag (no "rate us", no upsell).
- **Mashup:** macOS grouping + Windows actionable toasts + Linux "leave me alone" defaults.

### 4.9 System tray / menu — `desktop-shell.md §3`
- Tray icons **user-manageable** (show/hide/overflow popover — fixes the macOS Hidden-Bar
  and W11-secondary-monitor gripes). Optional global menu bar (macOS-style) available as a
  preset for the macOS crowd; off by default (per-window menus, Windows-style).

### 4.10 Settings — `settings.md` (authoritative)
- **ONE** unified Settings app. Every option discoverable via the search box at the top
  (fixes the Windows Settings-vs-Control-Panel split). Sidebar categories, breadcrumb,
  inline search highlighting. No nested dead-ends, no legacy dialogs.

### 4.11 File Manager — **TODO `file-manager.md`**
- **Purpose:** the modern Explorer/Finder RaeenOS promises.
- **Layout:** tabs + split/dual panes; sidebar (places, buckets, devices, cloud); fuzzy
  search; batch rename; cut/copy/paste with a clipboard history; path bar (editable);
  grid/list/column views; quick-look preview (Space). RaeFS per-app buckets surfaced.
- **Mashup:** Windows tabs + macOS column view + quick-look + Linux dual-pane (Krusader)
  power. Fixes both "Explorer from 2007" and Finder gaps.
- **Status:** `apps/files` is real (777 LOC, launchable) — audit against this spec + finish.

### 4.12 First-party app suite — `raeen-shell-apps`
Coherent, all on RaeUI tokens: **Files, Settings, Terminal, Text Editor, Calculator, Task
Manager** (shipped + launchable today), plus planned **Media, Photos, Browser (PWA shell)**.
Every app: same chrome, same shortcuts, same empty-states, same motion. *Cohesion is the
product* — this is how we beat Linux's inconsistency.

### 4.13 Lock / Login / OOBE — **TODO `oobe.md`** (partially in `setup_ui`)
- **OOBE:** the glass "Welcome to RaeenOS / set up your account" wizard — keep it warm,
  short, no dark patterns, **no account required** for local use (RaeID is optional). Pick
  the Familiarity Preset (§6) and Vibe here.
- **Login/Lock:** glass, wallpaper-aware, fast; passkey/biometric-ready (RaeShield).
- **Status:** OOBE renders on iron; must move fully to AA type + the design tokens.

### 4.14 GameOS Mode — **TODO `gameos.md`** (Concept §GameOS)
- Couch UI, big-picture, **controller-first**; instant toggle from the desktop (same OS,
  different shell). Big tiles, library, store, friends, performance overlay, one-button
  everything. Steam Big Picture / SteamOS as the bar.

### 4.15 Vibe Mode — **TODO `vibe-mode.md`** (Concept §Vibe Mode; `vibe_mode.rs` has 5 presets)
- System-wide visual *personalities* ("Cyberpunk Night", "Studio Ghibli Morning",
  "Bauhaus"): wallpaper + accent + system font + cursor + sound design + window-animation
  curves + RGB, applied as a **coherent set**. This is the customization Linux fans crave,
  made cohesive and one-click.

### 4.16 Multi-monitor — **TODO `multi-monitor.md`**
- Full taskbar + clock on **every** monitor (W11 fix); per-monitor scale + workspaces +
  wallpaper; sane window-to-display memory; mixed-DPI fractional scaling done right (the
  Linux fix). Hot-plug = no chaos.

### 4.17 Accessibility — **TODO `accessibility.md`** (Phase 19, ship gate)
- Screen reader (accessibility tree), magnifier, full keyboard-only navigation + visible
  focus order, high-contrast + reduced-motion themes, color-contrast compliance, hit
  targets ≥ spec. **Built in from the start, audited as a regression gate** — owned by
  `raeen-accessibility`.

---

## 5. Interaction model (one map across input methods)

- **Keyboard-first everywhere** — every action reachable without the mouse; shortcuts are
  consistent and discoverable (shown in menus + the command palette). Default hotkeys per
  preset (§6).
- **Mouse/trackpad:** drag-drop everywhere (pin, move, snap, file-onto-app); gestures
  (3/4-finger workspace + overview + back); hover previews; right-click = one flat complete
  menu (no "show more options").
- **Touch:** larger hit targets, swipe gestures, on-screen keyboard — RaeUI adapts.
- **Controller:** full navigation in GameOS Mode and, optionally, the whole shell.
- **Cursor:** hardware cursor plane (no lag); the input→photon budget is a perf gate
  (`raeen-perf`).

---

## 6. Familiarity Presets (the 10-minute switcher promise)

Chosen in OOBE, switchable in Settings. Each is a **coherent set**, not à-la-carte chaos:

| Preset | Taskbar | Window controls | Start/Search | Default WM |
|---|---|---|---|---|
| **Windows-like** | Bottom, Start left, grouped | Min/Max/Close **right** | Start-button + search-in-taskbar | Float + Snap Layouts |
| **macOS-like** | Dock bottom (magnify), global menu bar on | Traffic-lights **left** | Spotlight (Cmd+Space) | Float + Stage-style |
| **RaeenOS Native** | Bottom mica, Start center | Traffic-lights left | Command palette (Super+Space) | Hybrid tiling |

The underlying shell is the same; presets just set defaults. Everything stays tweakable.

---

## 7. Anti-patterns — the "we will never" list (burned into the EULA)

- ❌ Ads anywhere — Start, search, Explorer, lock screen, notifications. Ever.
- ❌ Telemetry without explicit, revocable, off-by-default opt-in.
- ❌ Forced updates or forced reboots (atomic update + one-click rollback instead).
- ❌ Bundled assistant / web-search injected into local search.
- ❌ Dark patterns in OOBE / settings (no "are you sure you don't want…", no buried opt-outs).
- ❌ Account required for local use.
- ❌ Inconsistent widgets — every surface uses RaeUI tokens, no exceptions.
- ❌ The 8×8 bitmap font on any user-facing surface.
- ❌ Locking customization away "for your own good."

---

## 8. Build order & status (ties to MasterChecklist)

**Already real (audit → polish → verify, not greenfield):** taskbar, Start, tray/quick-
settings, window chrome, notifications, the app suite (Files/Settings/Terminal/Text-Editor/
Calculator/Task-Manager), OOBE/login, Vibe-Mode presets, compositor glass/shadow/blur,
the AA type system. (See `goal-userspace-apps-status` audit.)

**Top gaps for "rivals Windows/macOS" (build these next):**
1. **Typography everywhere** — ensure every surface uses `draw_text_aa` (kill the bitmap
   fallback); this is the #1 "looks basic" fix.
2. **Global Search + Command Palette** (§4.4) — the signature feature.
3. **Window management** (§4.5) — snap/zones/tiling-resize protocol + full overview.
4. **Workspaces + Overview** (§4.6).
5. **File Manager** modernization (§4.11) to spec.
6. **Multi-monitor** correctness (§4.16) and **Accessibility** (§4.17, ship gate).

Each surface gets its own `docs/design/<surface>.md` (researched by `raeen-design-researcher`,
checked by `raeen-visual-qa` against macOS/Windows reference quality) before/with build.

---

## Sources (research foundation, §2)

- Windows 11 Start/taskbar complaints — [TechRadar: "so big it's basically a start screen again"](https://www.techradar.com/computing/windows/so-big-its-basically-a-start-screen-again-windows-11s-new-start-menu-is-getting-some-hate-and-triggering-windows-8-flashbacks), [TechRadar: taskbar's biggest flaw](https://www.techradar.com/news/microsoft-is-finally-fixing-the-windows-11-taskbars-biggest-flaw), [The Register: Start menu update](https://forums.theregister.com/forum/all/2025/05/07/microsoft_updates_the_windows_11/)
- macOS window-management / Finder / menu-bar gripes — [Blind: "I hate Mac"](https://www.teamblind.com/post/i-hate-mac-nqg1k5vp), [TileOrg: best macOS window managers](https://www.tileorg.com/blog/best-macos-window-managers/)
- Linux desktop fragmentation / inconsistency / HiDPI — [Wikipedia: Criticism of Linux](https://en.wikipedia.org/wiki/Criticism_of_Linux), [The Register: what the Linux desktop really needs](https://www.theregister.com/2025/12/22/what_linux_desktop_really_needs/), [DEV: why isn't Linux more popular on the desktop](https://dev.to/aghost7/why-is-linux-not-more-popular-on-the-desktop-40ln)
- Features users want — [OSnews: My Dream Operating System](https://www.osnews.com/story/16147/my-dream-operating-system/)
- Snap/tiling/overview prior art — [MakeUseOf: Snap Layouts vs macOS tiling](https://www.makeuseof.com/why-windows-11s-snap-layouts-beats-window-tiling-in-macos/), [It's FOSS: the rise of window tiling 2026](https://itsfoss.com/rise-of-window-tiling/), [HowToGeek: macOS-like Linux DEs](https://www.howtogeek.com/the-best-macos-like-linux-desktop-environments/)
- Design direction (Liquid Glass / glassmorphism discipline) — [UXPilot: glassmorphism best practices](https://uxpilot.ai/blogs/glassmorphism-ui), [DesignMonks: Liquid Glass UI](https://www.designmonks.co/blog/liquid-glass-ui), [Medium/Bootcamp: glassmorphism, the beautiful trap](https://medium.com/design-bootcamp/glassmorphism-the-most-beautiful-trap-in-modern-ui-design-a472818a7c0a), [Bookmarkify: 2026 UI trends](https://www.bookmarkify.io/blog/inspiration-ui-design)
