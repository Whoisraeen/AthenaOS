# Boot-Time Breakdown — AthenaOS (2026-06-22)

**Investigator:** raeen-perf | **Item:** CLAUDE.md live-fix #1 / Concept "fast is a feature: boot <6s, target 3s"
**Method:** parsed the saved integration-verify boot serial logs (measure-first; no boot-path code edited).

## Logs measured
- `C:\Users\woisr\AppData\Local\Temp\raeen-serial.log` (smp2, current %TEMP%) — contains TWO appended CI boots, both identical timing.
- `/tmp/raeen-serial-smp1.log`, `/tmp/raeen-serial-smp2.log` (integration verify captures).

All numbers below are **QEMU TCG** (the CI accel). Iron (Athena) is ~11.1s total; the iron/QEMU
difference is called out per cost center.

## Headline numbers (cite)
```
[BOOT-BENCH] T0 -> userspace = 14158 ms.  Concept target: <6000 ms (stretch 3000 ms).
[BOOT-BENCH] [WARN] Boot exceeded 6s target by 8158 ms.
```
T0 TSC armed at calibrated 4502 MHz. Total = **14158 ms**, budget 6000 ms -> **2.36x over**.

## Phase breakdown (sorted by cost)

Per-tier deltas computed from the `[TIER] ... (t=Nms)` cumulative timestamps:

| Phase (tier transition) | ms | % of 14158 | Classification |
|---|---:|---:|---|
| **T0 -> Tier 1 (core infra)** | 6031 | 42.6% | mixed; `modules=5574ms` sub-bucket dominates |
| **Tier 5 -> Tier 6 (USB/input)** | 3926 | 27.7% | **TCG-inflated** (xHCI HCE wedge + hub probes) |
| **Tier 7 -> Tier 8 (platform/misc)** | 1718 | 12.1% | mixed (`aer+modules` + many smoketests) |
| Tier 6 -> Tier 7 (power/ACPI) | 1033 | 7.3% | `aer+modules=1024ms` |
| Tier 3 -> Tier 4 (security) | 577 | 4.1% | crypto KATs (genuine, cheap-ish) |
| Tier 9 -> marker (post-init) | 525 | 3.7% | raefs/dhcp/demo |
| Tier 4 -> Tier 5 (procfs) | 140 | 1.0% | fine |
| Tier 2 -> Tier 3 (networking) | 115 | 0.8% | fine |
| Tier 8 -> Tier 9 | 87 | 0.6% | fine |
| Tier 1 -> Tier 2 (storage) | 6 | <0.1% | fine |

### Tier 1 sub-breakdown (from `[tier1-prof]`)
```
[tier1-prof] early=151ms acpi=242ms smp+sched=7ms pci=47ms fb=10ms modules=5574ms
```
**`modules=5574ms` is the single largest line item in the whole boot (39% of total).** This is the
block in `kernel/src/main.rs:623-700` — the Tier-1 service `init()`s **and their inline
`run_boot_smoketest()` calls**. The serial log between the config-registry line and the bootlog line
(~146 serial lines) is almost entirely feature smoketests: shell / settings / palette / spaces /
switcher / capture / wm / chrome / login / gfx / oobe and a **65-line notification smoketest storm**
(notify creates and tears down ~24 compositor surfaces, each a surface op + serial print).

### Tier 6 sub-breakdown (from `[tier6-prof]`)
```
[tier6-prof] input=2ms hid_init=0ms xhci_init=10ms xhci_smoke=639ms usb_core=1ms hid_smoke=3270ms msc_init=2ms msc_smoke=1ms
```
**`hid_smoke=3270ms` + `xhci_smoke=639ms` = 3909ms of USB enumeration.** Note: `hid_smoke` is a
**misattributed bucket** — `usb_hid::run_boot_smoketest()` itself is trivial (6 synthetic reports, no
waits; `kernel/src/usb_hid.rs:606`). The real cost in that window is the **deferred USB-hub
enumeration** the log shows at lines 746-798: per-port "power-good after 1000us" waits, downstream
device bring-up, and "GetPortStatus ... RingFull" failures, plus the xHCI controller wedging
(`USBSTS=0x00001008 HCE=true`, line 743) which then forces every subsequent probe through the
5ms-grace timeout path.

### Tier 7 sub-breakdown (from `[tier7-prof]`)
```
[tier7-prof] acpi+pcie=8ms usb=1ms aer+modules=1024ms
```
`aer+modules=1024ms` = the second full ACPI subsystem confirm + AER / PM / OOM / swap / soak / fatfs /
install / edid / thermal / suspend / power smoketests (log lines 801-891), again smoketest-bound.

---

## TOP cost centers

### 1. Tier-1 `modules` smoketest block — 5574 ms — GENUINE (helps iron) + DEFERRABLE
- **Site:** `kernel/src/main.rs:623-700` (the run of `*::init()` + inline `*::run_boot_smoketest()`).
- **Why slow:** dozens of correctness smoketests run **synchronously on CPU0 before the boot marker**.
  The notify smoketest alone creates/destroys ~24 compositor surfaces (83 surface+notify serial lines
  in the first boot). Each smoketest also emits multiple `serial_println!` lines; on iron each line is
  a byte-polled 115200 UART write **plus a GOP framebuffer blit** (~5ms/line per the
  iron-console-logging-tax memory note). ~146 serial lines in this region alone.
- **Classification:** GENUINE cost — helps BOTH QEMU and iron (the serial/framebuffer tax is *worse*
  on iron). On the critical path today but **architecturally deferrable**: ADR-0006
  (`kernel/src/boot_selftest.rs`) already exists and defers exactly this *class* of test (feature
  correctness that gates nothing the OS needs to be "up") to a post-marker sweep — but today it only
  moves **10** lightweight tests. The heavy ones (notify, settings, palette, spaces, switcher, capture,
  wm, chrome, gameos, oobe) are still inline.

### 2. USB enumeration (xHCI HCE wedge + hub probes) — ~3909 ms — TCG-INFLATED + cappable
- **Site:** `kernel/src/xhci.rs:2805 wait_for_transfer` (100ms / 5ms-grace timeouts) + the deferred
  hub-enum path (log lines 746-798); bucketed as `xhci_smoke`+`hid_smoke` in tier6-prof.
- **Why slow:** QEMU emulated xHCI **wedges** (HCE latches, line 743) enumerating the empty downstream
  ports of its USB3 hub. The existing HCE guard already cut this from ~9s to the current cost (5ms
  grace vs full 100ms — see the code comment + xhci-hce-grind-fix memory), but ~3.9s remains: every
  dead/wedged probe still eats the 5ms grace, hub ports each wait 1000us power-good, and the
  GetPortStatus RingFull retries re-poll.
- **Classification:** **Mostly TCG-INFLATED.** The code comment is explicit: on real hardware the
  controller does NOT wedge (HCE stays clear), so iron preserves the full 100ms-per-transfer path and
  this specific wedge cost largely **does not exist on iron**. Cutting it moves the **QEMU/CI number**
  far more than the iron number. (Iron USB cost is real but a different shape — real device round-trips,
  not dead-probe grace windows.)

### 3. Tier-7/Tier-8 `aer+modules` smoketest blocks — ~1024 ms + ~1718 ms — GENUINE + DEFERRABLE
- **Site:** `kernel/src/main.rs` Tier-7 (lines ~801-891) and Tier-8 (lines ~902-1056) module inits +
  inline smoketests (AER, PM, OOM, swap, soak, fatfs, install, EDID, compress, vgpu, crypto/TLS,
  raebridge, linuxkpi, bluetooth, etc.).
- **Why slow:** same pattern as #1 — correctness smoketests on the critical path, each emitting serial
  lines. Many gate nothing the OS needs to declare itself up.
- **Classification:** GENUINE (serial/compute tax helps iron too), DEFERRABLE via the same ADR-0006
  mechanism.

### 4. Serial + GOP-framebuffer mirror tax — cross-cutting amplifier
- **Site:** `console::_print` mirror of every `serial_println!` (active during boot, line 300
  "serial mirrored").
- **Why slow:** ~1153 serial lines in the first boot. Every line is a UART write + framebuffer blit.
  This is the multiplier that makes #1/#3 expensive — fewer/deferred smoketests = fewer lines = direct
  win. The iron-console-logging-tax memory already prescribes gating the serial->framebuffer mirror
  OFF at desktop activation; doing it *earlier* (before the smoketest storm) would help boot too, but
  that trades away on-screen boot diagnostics — lower priority than deferral.

---

## Recommended cuts (prioritized)

### CUT 1 (highest value, low risk) — Extend ADR-0006 deferral to the heavy Tier-1/7/8 smoketests
- **What:** move the non-critical feature smoketests (notify, settings, palette, spaces, switcher,
  capture, wm, chrome, gameos couch/glyph/padbind/gamebar/profile/osk, clipboard-panel, oobe layout,
  AER / OOM / swap / soak / install / edid / compress / vgpu / raebridge correctness checks) from
  inline calls in `main.rs` into `boot_selftest::run_deferred()` so they run **post-marker**. Keep
  every `*::init()` (anything that spawns a thread or seeds state the OS depends on) on the critical
  path; defer only the `run_boot_smoketest()` *checks* — exactly what ADR-0006 was built for.
- **Expected savings:** the bulk of `modules=5574ms` + `aer+modules` (1024+1718). Conservatively
  **3000-4500 ms off the BOOT-BENCH number** (the tests still run + still print PASS/FAIL, just after
  the marker). Helps iron MORE than QEMU (serial tax is heavier on iron).
- **Risk:** LOW. Mechanism + reversal already documented in `boot_selftest.rs`. The only correctness
  rule: a deferred test must not be one the boot-health 7/7 aggregation depends on, and its `init()`
  must already have run. (Watch the notify smoketest: confirm it does not leave surfaces the first real
  composite needs — it tears its own surfaces down, so safe.)
- **Owner:** **raeen-kernel** (boot-path ordering in `main.rs` + `boot_selftest.rs`).
- **Proof markers:** lower `[BOOT-BENCH] T0 -> userspace`; `[boot-selftest] deferred sweep DONE: N`
  with N grown from 10; every moved test STILL prints its `-> PASS`; no lost `[ OK ]` lines;
  `[tier1-prof] modules=` drops sharply.

### CUT 2 (high value for CI, low risk) — Tighten the xHCI dead-probe path
- **What:** once the controller has latched HCE (`host_error_logged` already tracks first observation,
  `xhci.rs:2859`), short-circuit subsequent `wait_for_transfer` / hub `GetPortStatus` probes to fail
  immediately (or a sub-ms grace) instead of re-paying the 5ms grace per dead probe; and skip the
  per-port 1000us power-good wait for ports that report no connection. Cap total hub-enum probe budget.
- **Expected savings:** most of the ~3.9s Tier-6 USB cost **on QEMU/CI** — plausibly **2000-3000 ms
  off the BOOT-BENCH number**.
- **Risk:** LOW-MEDIUM. Must preserve the "keep polling for an in-flight completion" property the
  existing comment guards (a real downstream-hub HID config event can be microseconds away). Gate the
  fast-fail strictly on HCE/HSE latched, never on a clean controller, so **iron behaviour is
  unchanged** (controller never wedges there).
- **Owner:** **raeen-drivers** (xHCI / USB hub enumeration).
- **Proof markers:** `[tier6-prof] hid_smoke`/`xhci_smoke` drop; the 5x `GetPortStatus ... RingFull`
  and dead-probe lines shrink; 3 HID interrupt-IN endpoints STILL armed (no lost input devices);
  `[TIER] Tier 6 complete` timestamp drops; iron tier6-prof unchanged on next flash.

### CUT 3 (medium value, helps iron most) — Gate the serial->framebuffer mirror earlier
- **What:** disable the GOP-framebuffer mirror of `serial_println!` before the smoketest-heavy tiers
  (keep UART + bootlog ring + netlog, which are durable), per the iron-console-logging-tax memory.
- **Expected savings:** secondary; compounds CUT 1/3 by removing the blit per line. On **iron** this is
  the bigger lever (each line ~5ms there).
- **Risk:** LOW but trades on-screen boot diagnostics during the smoketest window. Lower priority than
  CUT 1.
- **Owner:** **raeen-kernel** (console mirror gate).
- **Proof markers:** lower iron BOOT-BENCH specifically; no change to captured serial/bootlog content.

---

## QEMU-TCG vs iron honesty note
- **CUT 1 and CUT 3 move the REAL (iron) number** — they remove genuine serial/compute work that iron
  also pays (and pays more, via the 5ms/line UART+blit tax).
- **CUT 2 moves mostly the QEMU/CI number** — the xHCI HCE wedge is a QEMU emulation artifact; iron's
  controller does not wedge, so iron USB cost has a different shape (real device round-trips). Worth
  doing for fast CI, but do not expect it to fix the iron 11.1s on its own.
- The iron 11.1s breakdown CANNOT be confirmed from these QEMU logs. **Iron check: PENDING iron
  resume** — the kernel already emits `[tier1-prof]` / `[tier6-prof]` / `[tier7-prof]`, so the next
  Athena flash will localize the iron number with the same buckets. Hypothesis from the comments: on
  iron the ACPI namespace parse (159 devices vs 54 in QEMU) and the serial/blit tax dominate, so CUT 1
  + CUT 3 should help iron the most.

## Safe-to-land-immediately assessment
**CUT 1 is the one safe enough to recommend landing first** — it reuses an existing, documented,
reversible mechanism (ADR-0006), changes only *when* tests run (not *whether* they can FAIL), and has
the largest expected savings. It must still go through raeen-kernel + raeen-verifier (boot >=5x at
SMP=1 and SMP=2 per CLAUDE.md rule 17; confirm no PANIC, marker present, all moved tests still print
PASS, BOOT-BENCH lower). I did not edit the boot path this slice.
