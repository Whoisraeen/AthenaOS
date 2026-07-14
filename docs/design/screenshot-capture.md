# Design Spec: Screenshot & Region Capture (with markup)

> *"Built for people who care about how things feel."* — LEGACY_GAMING_CONCEPT.md
>
> A hotkey dims the screen; you drag a rectangle, the dimensions tick up live, you
> let go, and an action bar offers copy / save / markup / pin. It must clear:
> **macOS Screenshot (`Cmd+Shift+4/5`) and Flameshot** — instant region capture
> with an in-flow markup toolbar, not a round-trip to an editor.

**All tokens below are defined in [`design-language.md`](./design-language.md)**
and live in `rae_tokens` (ADR 0003). This spec only *assigns* them. No new magic
numbers; surface-specific layout dimensions are local constants from `space.*`.

---

## Concept promise + bar to clear

> "Built for creators… capture & stream at the compositor — zero-cost recording,
> no OBS overhead." — `capture.rs` module doc / Concept §creators.

- **Bar to clear:** macOS Screenshot (region drag with a live size readout, the
  floating thumbnail → markup, copy-vs-save) **and** Flameshot (the in-flight
  toolbar of pen/arrow/rect/text/blur right on the dimmed overlay). AthenaOS does
  both **reading pixels straight from the compositor** — no app window, no
  off-screen round trip.

---

## Already built (delta only — verify-before-spec)

Grounded in code. **Both the capture engine and the tool/markup model exist;** the
job is to *wire the tool to the compositor's real pixels* and render the overlay —
not to build capture or markup from scratch.

| Piece | Where | Today | This spec adds |
|---|---|---|---|
| **Compositor capture engine** | `kernel/src/compositor.rs::start_capture(rx,ry,rw,rh,format,continuous)->u64`, `read_capture(id)->Option<(Vec<u32>,w,h)>`, `stop_capture(id)`, `CaptureSession`, `CaptureFormat` | **LIVE** — region capture reads real composited pixels off the front buffer | the overlay calls `start_capture` for the selected region and `read_capture` to get the pixels (**wire to this; do not rebuild**) |
| Screenshot tool (mode + flow) | `components/raeshell/src/screenshot.rs::ScreenshotTool` | LIVE data model, **unwired** (`allow(unused)`): `CaptureMode` (FullScreen/ActiveWindow/Rect/Freeform/Scrolling/Delayed), `start_capture`/`finish_capture`/`cancel`, `SelectionRegion` (drag + resize handles + state machine), `SnippingToolbar`, history, pin, keybindings | the dimmed overlay + action bar rendered over it, fed by the compositor pixels |
| Selection state machine | `screenshot.rs::SelectionRegion { state, x, y, width, height, anchor }` + `SelectionState` (Selecting/Selected/Moving/Resizing*) | LIVE | drives the §3 rectangle visuals + handles |
| Markup model | `screenshot.rs::Annotation` + `AnnotationKind` (Pen/Arrow/Rectangle/Ellipse/Text/Blur/Pixelate/Highlight/NumberMarker), `AnnotationProperties` (line width, color, fill, opacity), undo | LIVE | the §5 markup toolbar drives these; rendering them onto the captured image |
| Post-capture actions | `save_current` (→ history), `pin_current` (→ `PinnedScreenshot`, always-on-top), clipboard via `OutputSettings.auto_copy_clipboard` | LIVE | the §4 action bar buttons map to these |
| Userspace `CaptureEngine` (recording/stream) | `components/raeshell/src/capture.rs` | LIVE (recording/stream + on-screen REC indicator) | **separate concern** — that's video capture; this spec is *still-image* capture. They share the compositor source but are distinct surfaces. |
| Glass / shadow / dim | `compositor::set_surface_blur`, `SurfaceEffect::DropShadow` | LIVE | action bar = `material.glass` + `elev.3`; overlay dim = a scrim |

**This is a wire-up, not a rebuild.** The compositor already hands back real
region pixels; the tool already models modes, the selection drag, the full markup
set, and the save/pin/clipboard actions. Missing: (a) the global hotkeys bound to
`ScreenshotTool::handle_key` driving the **real** compositor capture, (b) the
dimmed-overlay + selection visuals, (c) the post-capture action bar UI.

---

## Prior art distilled (current systems, 2024–2025)

- **macOS Screenshot (`Cmd+Shift+4`, `…+5`):** crosshair cursor with a **live
  px×px readout** floating beside it; `Space` toggles window-capture (highlights
  the window under the cursor); after capture a **thumbnail floats bottom-right**
  for a few seconds — click it to mark up, ignore it and it saves. `…+5` adds a
  mode bar (full/window/region + record + options). **Take:** the live-dimensions
  readout, the window-pick mode, the "thumbnail → optional markup, else auto-save"
  flow, the mode bar. **Avoid:** the disappearing-thumbnail timing being too
  short (ours keeps the action bar until dismissed).
- **Flameshot (Linux):** on capture, the dimmed overlay shows an **inline toolbar**
  (pen, line, arrow, box, circle, marker, blur, text, numbered steps, color
  picker) right there — annotate before you ever leave the capture. Copy/save/pin
  are one click. **Take:** the **in-flight markup toolbar** (this is the headline
  feature — no editor round-trip) and the blur-for-redaction tool. **Avoid:** the
  toolbar's slightly cluttered density — we group tools and use the token spacing.
- **Windows 11 Snipping Tool:** top mode bar (rectangle/window/full/freeform +
  delay), then opens an editor window for markup; copies to clipboard + toast.
  **Take:** the delay timer + freeform mode (both already in `CaptureMode`).
  **Avoid:** the *separate editor window* — AthenaOS marks up **on the overlay**
  (Flameshot model), no context switch.

**AthenaOS synthesis:** macOS's **live-dimensions crosshair + window-pick + mode
bar**, Flameshot's **in-flight markup toolbar + blur redaction**, Snipping Tool's
**delay/freeform modes** — all rendered as a glass overlay that reads real
compositor pixels and the live accent.

---

## AthenaOS design tokens this surface uses

Pulled verbatim from `design-language.md` / `rae_tokens`. No new magic numbers.

- **spacing:** `space.1` (tool-button gaps), `space.2` (toolbar/action-bar inset,
  handle offset), `space.3` (button padding), `space.4` (bar internal padding).
- **radius:** `radius.lg` (the markup toolbar + action-bar panels), `radius.sm`
  (tool buttons, the dimensions-readout pill is `radius.pill`), `radius.xs`
  (selection-handle squares, color swatches).
- **elevation:** `elev.3` (the markup toolbar + action bar — transient panels over
  the overlay), `elev.focus` (focused tool button glow).
- **type:** `type.label` (tool button labels / tooltips), `type.caption`
  (dimensions readout "1280 × 720", action-bar button labels, hints).
- **accent model:** seed `ThemeAbi.accent_argb` → `derive_accent`. The **selection
  rectangle border = `accent.base`**, handles = `accent.base`, the active markup
  tool = `accent.subtle` fill + `accent.text` glyph, the dimensions pill text =
  `accent.text`. **No private `const ACCENT`** (today `screenshot.rs` hardcodes
  `SELECTION_BORDER = 0xFF_4E_9C_FF` — that gets deleted for the token).
- **material:** `material.glass` (the toolbar + action bar — small transient).
- **motion:** `motion.fast` (action bar appears after capture; toolbar slide-in),
  `motion.micro` (tool hover/select, handle grab), `motion.exit` (dismiss /
  copy-and-close), `motion.instant` (reduced-motion).

### Overlay scrim (NEW token — proposed addition to `design-language.md`)
The dimmed-overlay backdrop needs a defined value. Propose adding to §4 color:

> `scrim.capture` = `0x99_06_08_10` — ~60% near-black dim applied over the whole
> screen during region selection; the *selected* region is punched back to full
> brightness (un-dimmed) so the user sees exactly what they'll capture.

Today `screenshot.rs` uses `OVERLAY_DIM = 0x88_00_00_00`. The proposed
`scrim.capture` standardizes it (tinted toward `bg.base` rather than pure black,
for material consistency). **This is the one new token this spec introduces; add
it to `design-language.md` §4 before referencing.** (Noted in DESIGN_LANGUAGE
update list below.)

---

## 1. Invocation

The hotkeys map to the existing `KeyBindingManager` defaults in `screenshot.rs`
(re-expressed with the AthenaOS `Super`-first convention; the manager is rebindable):

- **Region capture:** `Super+Shift+S` (matches Windows muscle memory; already the
  `CaptureRegion` default keycode). Dims the screen → crosshair drag.
- **Full screen:** `PrintScreen` (already `CaptureFullScreen`).
- **Active window:** `Alt+PrintScreen` / `Super+Shift+W` (already `CaptureActiveWindow`).
- **Freeform:** `Super+Shift+F` (already `CaptureFreeform`).
- **Mode bar (macOS `…+5`):** `Super+Shift+T` opens the `SnippingToolbar` to pick a
  mode before capturing.
- **Delay:** the Delayed3s/5s/10s modes (already in `CaptureMode`) are reachable
  from the mode bar; a `type.caption` countdown shows on the overlay.

All routes call `ScreenshotTool::handle_key` → `start_capture(mode)`. The
**actual pixels** come from the compositor's `start_capture`/`read_capture` for
the resolved region (the wire-up this spec demands).

---

## 2. Capture modes

| Mode | `CaptureMode` | Overlay behavior |
|---|---|---|
| **Region (drag)** | `RectangularRegion` | full-screen dim + crosshair; drag a rectangle (§3) |
| **Window** | `ActiveWindow` | hover highlights the window under the cursor with an `accent.subtle` outline + dim everything else; click captures it (macOS `Space` model) |
| **Full screen** | `FullScreen` | immediate capture of the whole composited frame; jumps straight to the action bar |
| **Freeform** | `FreeformRegion` | lasso path (`SelectionRegion.freeform_points`); dim outside the path |
| **Delayed** | `Delayed3s/5s/10s` | a `type.display` countdown center-screen, then the chosen mode fires (`tick_delay`) |

---

## 3. Dimmed-overlay + selection rectangle visuals

**Bar to clear:** macOS's crosshair + live readout; Flameshot's clean selection.

- **Backdrop:** the whole screen dims to `scrim.capture` (the proposed token).
- **Selected region:** punched back to **full brightness** (un-dimmed) so the user
  previews the exact capture. A **1px `accent.base` border** frames it (replacing
  the hardcoded `SELECTION_BORDER`).
- **Crosshair:** thin full-height + full-width `stroke.subtle` guide lines through
  the cursor while in `Selecting` (the `CROSSHAIR_COLOR` today → `stroke.subtle`).
- **Resize handles:** 8 small `radius.xs` `accent.base` squares (corners + edge
  midpoints) once `state == Selected` — they drive `SelectionState::Resizing*`.
  Each handle is an 8px visual with a ≥16px grab area.
- **Dimensions readout:** a `radius.pill` `material.glass` pill that **follows the
  cursor / sits just outside the selection's top-left**, showing live
  "`<w> × <h>`" (`type.caption`, `accent.text`) updating every drag frame
  (`SelectionRegion.update` already recomputes w/h). When the region is small it
  flips to inside-the-region so it's never clipped off-screen (macOS behavior).
- **Magnifier loupe (optional, deferred):** a small zoomed pixel-grid near the
  crosshair for pixel-precise edges (macOS has it) — flagged as a v2 nicety.

### States
- **Idle → Selecting:** crosshair + dim, no rectangle yet.
- **Selecting (dragging):** rectangle grows, readout updates, no handles yet.
- **Selected:** handles appear, the markup toolbar + action bar fade in
  (`motion.fast`).
- **Moving / Resizing:** the rectangle tracks; readout updates.
- **reduced-motion:** the bars appear instantly (opacity-only); the rectangle
  still tracks the drag (functional, not decorative).
- **dark/light:** the scrim is palette-neutral; bars use glass tint per palette.

---

## 4. Post-capture action bar

Appears anchored just below (or above, flipping to stay on-screen) the selected
region the moment `state == Selected`. **Bar to clear:** macOS thumbnail actions +
Flameshot's one-click copy/save/pin.

- **Surface:** `material.glass`, `radius.lg`, `elev.3`, `space.2` inset,
  horizontal row of buttons.
- **Buttons** (each 36px, `radius.sm`, `type.label`, icon + optional label):
  - **Copy** → set the captured image on the clipboard (`OutputSettings.auto_copy_clipboard`
    path → kernel clipboard; the clipboard-history image entry, per
    `clipboard-history.md`). Default primary action; `Enter` fires it.
  - **Save** → `save_current` (→ `ScreenshotHistory` + file). Shows the saved path
    in a brief `state.ok` toast.
  - **Markup** → toggles the §5 markup toolbar (Flameshot in-flight editing).
  - **Pin** → `pin_current` (`PinnedScreenshot`, always-on-top floating image —
    the macOS "keep it on screen to reference" trick).
  - **Close / Discard** (`state.danger` hover) → `cancel`.
- **States:** button hover `bg.elevated`; active `accent.subtle`; focus ring +
  `elev.focus`; the primary (Copy) button is `accent.subtle` resting to signal
  the default.

---

## 5. Markup tools (in-flight, Flameshot-style)

A vertical (or horizontal, screen-fit) `material.glass` toolbar (`radius.lg`,
`elev.3`) adjacent to the selection, driving `ScreenshotTool.current_annotation_kind`
+ `AnnotationProperties`. **The basic set this spec requires** (all already in
`AnnotationKind`):

| Tool | `AnnotationKind` | Notes |
|---|---|---|
| **Pen** (freehand) | `Pen` | `points` path; line width from props |
| **Arrow** | `Arrow` | start→end; the call-out workhorse |
| **Rectangle** | `Rectangle` | outline/filled per `FillMode` |
| **Text** | `Text` | click to place, type; `font_size` from props |
| **Blur** (redaction) | `BlurRegion` | reuse the compositor blur over the region — the privacy tool; `Pixelate` is the alt |
| (bonus, already modeled) | `Ellipse` / `Highlight` / `NumberMarker` | surface if space allows |

- **Tool button states:** default `bg.elevated`; **active tool** = `accent.subtle`
  fill + `accent.text` glyph (only one active); hover `bg.overlay`; focus ring +
  `elev.focus`. 32px hit targets.
- **Property strip:** below the tools — a **color swatch row** (drives
  `AnnotationProperties.color`; the existing `ColorPicker` + eyedropper) and a
  **line-width stepper** (`set_line_width`, clamped 1–10). Color swatches are
  `radius.xs`; the active swatch gets an `accent.base` ring.
- **Undo:** `Ctrl+Z` → `undo_annotation` (`remove_last_annotation`). A visible
  undo button too.
- **Annotations render onto the captured image** (`CapturedImage.annotations`) so
  Copy/Save/Pin all include the markup.

---

## 6. Keyboard + controller navigation map

- Invocation hotkeys per §1 (via `KeyBindingManager`, rebindable in Settings).
- During selection: `Esc` cancels (`cancel`); `Enter` confirms the current
  selection → action bar; arrow keys nudge the selection edge by 1px (10px with
  `Shift`) for precision.
- `Space` (in Region mode) toggles to window-pick (macOS parity).
- Action bar: `C` copy, `S` save, `M` markup, `P` pin, `Esc` discard; `Tab` cycles
  buttons with a focus ring.
- Markup: number/letter shortcuts per tool (`P`en, `A`rrow, `R`ect, `T`ext,
  `B`lur); `Ctrl+Z` undo.
- **Controller (couch):** stick moves the crosshair, A starts/ends the drag,
  bumpers cycle tools, X copy, Y save, B cancel; bars use the 48px couch floor.

---

## 7. Accessibility (in scope from the start)

| Concern | Rule | Owner |
|---|---|---|
| Contrast | dimensions readout `accent.text` ≥4.5:1 on the glass pill; tool labels ≥4.5:1; the selection border must stay visible over *any* captured content (1px accent + a 1px `stroke.strong` inner line for contrast on bright regions) | raeen-accessibility |
| Focus visibility | active tool + focused button = `accent.subtle`/`accent.base` ring + `elev.focus`, never color-only | raeen-accessibility |
| Reduced-motion | bars appear instantly; no toolbar slide; the live readout still updates (functional) | raeen-accessibility |
| Hit targets | tool + action buttons 32px (pointer) / 48px (couch); resize handles ≥16px grab area | raeen-visual-qa |
| Keyboard-complete | full capture → markup → copy/save reachable with no pointer (arrow-nudge selection + tool shortcuts) | raeen-accessibility |
| Redaction trust | the **Blur/Pixelate** redaction must be applied to the saved pixels, not just visually overlaid — flag so a "blurred" secret can't be recovered from the file | raeen-accessibility + AthGuard |

---

## 8. Cohesion acceptance

Ships only when:
1. The selection border, handles, active-tool fill, and dimensions readout all use
   the **same `accent.base`/ramp** as the shell — Vibe-switch and the capture
   overlay re-tints with the taskbar/Start (kills the hardcoded
   `SELECTION_BORDER`/`HANDLE_COLOR` in `screenshot.rs`).
2. The toolbar + action bar use `material.glass`, `radius.lg`, `elev.3` — the same
   transient-glass family as the command palette and clipboard panel.
3. Active tool ≠ hover visually.
4. Dark + light both pass contrast; the scrim reads on both.

---

## Handoff

### Implementers (two-part — the capture wire-up then the UI)
- **raeen-gfx** — the **capture wire-up**. Connect `ScreenshotTool`'s resolved
  region/mode to the compositor's real pixels: call
  `compositor::start_capture(rx,ry,rw,rh, CaptureFormat::*, /*continuous=*/false)`
  for the selection, `read_capture(id)` to get `(Vec<u32>, w, h)`, feed it to
  `ScreenshotTool::finish_capture(pixels, w, h, ts)`, then `stop_capture(id)`.
  Confirm `CaptureFormat` covers the ARGB/BGRA the markup expects; render the
  selection rectangle, handles, crosshair, and the un-dimmed region punch on the
  overlay via `Canvas`.
- **raeen-shell-apps** — the **UI surface**. Bind the global hotkeys to
  `ScreenshotTool::handle_key` (driving the **real** capture above, not the stub),
  render the §3 dimmed overlay, the §4 action bar, and the §5 markup toolbar +
  property strip; map the action buttons to `save_current` / `pin_current` /
  clipboard; **delete the hardcoded color consts** in `screenshot.rs`
  (`SELECTION_BORDER`, `HANDLE_COLOR`, `OVERLAY_DIM`, `TOOLBAR_*`) and consume
  `rae_tokens` + the proposed `scrim.capture`.
- **raeen-design-researcher (me)** — add `scrim.capture` to
  `design-language.md` §4 (the one new token) — see DESIGN_LANGUAGE update note.
- **raeen-accessibility + AthGuard** (flagged) — redaction-applied-to-pixels
  guarantee; contrast of the border/readout over arbitrary content.

### FAIL-able boot-log proof line
From a `run_boot_smoketest` that drives a synthetic region capture through the
**real** compositor path (must print FAIL):

```
[screenshot] smoketest: region=200x100 captured_px=20000 annot=arrow+blur copy_ok=1 save_path=/home/screenshots/.. -> PASS
```

(FAIL if `read_capture` returns no pixels / wrong dimensions for the requested
region, if an annotation is not present on the saved `CapturedImage`, if the blur
redaction is not baked into the saved pixels, or if copy/save returns an error.)
Plus cohesion:
`[screenshot] accent=0x.. == derive_accent(seed).base (no hardcoded border) -> PASS`.

### Visual-QA verification list (raeen-visual-qa)
- QEMU screenshot: region-capture overlay mid-drag — screen dimmed to
  `scrim.capture`, the selected region un-dimmed, **1px accent border**, 8 handles,
  and the **live dimensions pill** ("`W × H`") in the accent.
- Screenshot: the **action bar** below a finished selection — Copy (accent-primary)
  / Save / Markup / Pin / Close, glass material.
- Screenshot: the **markup toolbar** active — pen/arrow/rect/text/blur with one
  tool selected (accent fill), color swatch row, line-width stepper; an arrow + a
  blurred region drawn on the capture.
- Screenshot: window-pick mode highlighting a window with the accent outline.
- Cohesion: before/after Vibe switch — selection border + handles + active tool
  re-tint with the shell.

### Unblocks (MasterChecklist)
- Phase 6 (AthGFX): exercises the compositor capture path end-to-end (real region
  pixels → image).
- Phase 8 (AthUI/AthKit): the glass-toolbar + tool-button widgets.
- Phase 14 (AthShell + apps): the screenshot/markup surface; activates the dead
  `raeshell::screenshot` tool and connects it to live capture.
