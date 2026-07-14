# RaeenOS Input Pipeline

Input latency is half the gaming-first promise (the other half is frame pacing). This doc
maps the path a physical action takes — keypress, mouse move, gamepad stick — from
silicon to the game, and where each stage stands. Latency budgets live in
`PERFORMANCE_TARGETS.md` §4; this is the architecture.

---

## 1. The path (silicon → game)

```
 device  ──IRQ──▶  driver        ──▶  input.rs           ──▶  routing        ──▶  consumer
 (HW)              (decode)            (normalize event)       (focus/grab)        (SCHED_GAME)

 USB kbd/mouse/pad ─▶ xhci + usb_hid ─┐
 PS/2 kbd/mouse    ─▶ i8042 (planned) ─┼─▶ InputEvent ─▶ focus-aware dispatch ─▶ game / compositor / shell
 gamepad (DS/Xbox) ─▶ usb_hid parsers ─┘                                          (woken with priority)
```

**Principle:** input is **IRQ-driven**, never polled by a busy loop, and the consuming
game thread is **SCHED_GAME** so an event preempts normal work. No stage may batch on the
game path to "save interrupts" — that trades latency we refuse to spend.

---

## 2. Stages

### 2.1 Transport drivers
- **USB HID** (`usb_hid.rs`, over `xhci.rs`): keyboard, mouse, gamepads. Boot protocol +
  report protocol. 1000 Hz target poll interval. *(written; being hardware-debugged on
  Athena — hub-stall path, see the USB/HID debug notes.)*
- **PS/2 i8042** (planned, greenfield): keyboard/mouse fallback + the QEMU input path. The
  reference native driver in `NATIVE_DRIVER_PLAN.md`.
- **USB hub** (planned, greenfield): required for real-world multi-device port trees —
  currently the Athena input blocker.

### 2.2 Decode → normalized `InputEvent`
Each driver turns raw reports into a uniform `InputEvent` (`input.rs`): key down/up with a
stable keycode, relative/absolute pointer deltas, button/axis state. Scancode-set and
HID-usage translation happen here so downstream never sees device-specific encodings.

### 2.3 Gamepad specifics (gaming-first)
- **DualSense / Xbox parsers** exist (`input.rs`): buttons, sticks, triggers, with
  per-controller report layouts.
- **Output reports**: `DualSenseOutput::build_report` (rumble, LED, trigger effects) —
  the haptics/feedback path back to the pad.
- **To wire:** stick deadzones + response curves, per-game remapping, battery/connection
  state, and the Settings surface (`SETTINGS_CATALOG.md` §4: test, remap, deadzones,
  rumble, LED).

### 2.4 Key remapping
`components/kanata_daemon` (vendored kanata-keyberon) provides advanced key remapping
(layers, tap-hold, combos) as a userspace daemon — RaeenOS's "PowerToys Keyboard Manager /
karabiner" equivalent. Sits between normalize and dispatch for keyboard events.

### 2.5 Routing / dispatch
Focus-aware delivery: keyboard → focused window; pointer → window under cursor (or the
grabbing surface); gamepad → the foreground game (or the shell in GameOS/couch mode).
Exclusive **grab** (pointer lock for FPS, raw input) bypasses cursor acceleration and
delivers raw deltas. Global hotkeys (Game Bar, screenshot) are intercepted pre-dispatch.

### 2.6 Consumer wakeup
The target thread is **SCHED_GAME**; delivery wakes it with real-time priority so the
input→game latency is a scheduler wakeup, not a tick boundary. This is the stage that most
needs telemetry (`PERFORMANCE_TARGETS.md` §4/§5 list it ⬜ — wakeup latency isn't
instrumented yet).

---

## 3. Accessibility & alternative input (parity)

Hooks the pipeline must expose (see `SETTINGS_CATALOG.md` §11): sticky/filter/toggle keys,
mouse keys (pointer via keypad), on-screen keyboard, dwell-click, and voice control. These
live as transforms between normalize and dispatch so they apply uniformly across all
devices. *(Planned.)*

---

## 4. Status & gaps

| Stage | Status |
|---|---|
| USB HID kbd/mouse | 🟡 written, Athena hardware-debugging |
| Gamepad parse (DualSense/Xbox) | 🟡 parsers + output report exist |
| PS/2 i8042 | ⬜ greenfield (reference native driver) |
| USB hub | ⬜ greenfield (Athena input blocker) |
| Normalize → InputEvent | 🟡 |
| Key remapping (kanata) | 🟡 daemon vendored |
| Focus routing / grab / hotkeys | 🟡 |
| SCHED_GAME wakeup + latency telemetry | ⬜ not instrumented |
| Deadzones / curves / per-game remap | ⬜ |
| Accessibility transforms | ⬜ |

**Two concrete next steps:** (1) the **USB hub** driver unblocks real Athena input; (2) a
**`/proc/raeen/input`** latency counter (report-arrival → event-dispatch → consumer-wake)
turns the ⬜ latency rows measurable. Both follow the `NATIVE_DRIVER_PLAN.md` method and the
`TESTING_STRATEGY.md` ladder.
