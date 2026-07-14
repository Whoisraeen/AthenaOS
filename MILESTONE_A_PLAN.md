# Milestone A — Daily Driver: Dependency-Ordered Execution Plan

**Goal:** reach **G1 (Daily Driver)** from `PRODUCTION_CHECKLIST.md` — a machine the owner can install, log into, network, file-manage, configure, run apps on, and live in without a terminal.

**Anti-stale contract:** this is an *execution plan*, not a status owner. Every item cites its `MasterChecklist.md` line; **MasterChecklist owns the `[x]`/`[~]`/`[ ]`**. When they disagree, MasterChecklist wins and this plan is stale — re-derive it. Precedence: `RaeenOS_Concept.md` > `MasterChecklist.md` > this file.

---

## The strategic frame: v0.1 lean, then fast-follow updates

The fastest route to *actually daily-driving* is **not** the full G1 bar. It is a deliberately reduced **v0.1 "Dogfood Daily Driver"** that ships the irreducible core, with the four mountains delivered as **post-v0.1 updates**.

### Why deferral is safe here (the architecture earns it)

Three of the four long poles are **userspace components**, so they ship as package/daemon updates with **no kernel reflash**:

| Deferred feature | Where it lives | Delivery vehicle |
|---|---|---|
| AMD GPU acceleration | `amdgpud` userspace daemon (LinuxKPI Path C) | userspace package update |
| Wi-Fi | `iwlwifi` userspace LinuxKPI port | userspace package update |
| Web browser | userspace app (`raeweb` / native engine) | RaeStore / package update |
| S3 sleep/wake | **kernel-side** | A/B kernel slot update (Phase 3.6) |

This is exactly the Concept model — *"atomic CoW updates that never half-apply"* (§Core 2). The SW compositor renders the desktop today on iron; GPU/Wi-Fi/browser layer on top without rearchitecting. The only kernel-side deferral (S3) rides the A/B slot mechanism you've already built.

> Chicken-and-egg note: OTA update + rollback (Phase 3.6 iron half) is itself deferrable for v0.1 — while dogfooding on your own machine you iterate by **reflashing the USB**, not OTA. OTA only becomes load-bearing when shipping to *other* people. So v0.1 can defer it and still iterate freely.

---

## v0.1 ship cut (the fastest dogfood-able build)

**IN v0.1:**
- Install to internal NVMe, reboot into installed OS, persistent RaeFS
- Live USB keyboard + mouse on iron
- **Software-composited** desktop at 1080p (taskbar, start, alt-tab, notifications, search)
- Files (browse real RaeFS), Settings (change + persist wallpaper), Terminal, native apps
- **Ethernet** networking → DHCP Bound → DNS
- Clean shutdown / reboot, no data loss

**DEFERRED to fast-follow updates (with the cost of deferring):**
- **GPU acceleration** → SW raster caps resolution/perf, costs battery. *Update 1.*
- **Wi-Fi** → tethered to Ethernet until then. *Update 2.*
- **S3 sleep/wake** → power-off instead of sleep; laptop-unfriendly. *Update 3.*
- **Full browser** → **DECIDED 2026-06-15: deferred to Update 4.** No full browser in v0.1 (optional trivial link-opener only); web on another machine until then. The engine choice (Servo-class native vs minimal web view) is itself deferred to Update-4 scheduling. ⚠️ the plan flags this as the hardest gap to live without — accepted for the fastest dogfood-able v0.1.
- **OTA update + rollback, multi-monitor, HDR/VRR, 24h-soak sign-off** → release-gate items, not dogfood blockers.

---

## Dependency graph (waves)

```
WAVE 0  Foundation fixes (iron) ─── gates everything interactive
  AP post-boot scheduling · compositor IF=0 deadlock (iron verify) · xHCI HCE grind (done)
        │
        ▼
WAVE 1  Live interaction on iron
  WS1 Live HID  ──────────────┐
  WS2 Ethernet→Bound (parallel, independent)
        │                     │
        ▼                     │
WAVE 2  Persistent + usable   │
  WS3 Install-to-disk (needs WS1 for wizard UI) │
  WS4 Files/Settings usable (needs WS1) ◄───────┘
        │
        ▼
WAVE 5  Release hardening (soak, boot time, multi-SKU)

WAVE 3  THE MOUNTAINS — start NOW in parallel, independent of Waves 0-2
  WS5 GPU bring-up · WS6 Browser
        │
        ▼
WAVE 4  Round out daily use (post-v0.1 updates)
  WS7 Wi-Fi · WS8 S3 sleep · WS9 OTA update+rollback
```

**Critical path to v0.1:** Wave 0 → Wave 1 → Wave 2. These are **verification-bound** (flash-and-fix), not greenfield.
**Critical path to full G1:** Wave 3 (GPU + browser) — the actual calendar driver. Start it in parallel on day one.

---

## Wave 0 — Foundation fixes (prerequisite for all interaction)

These are iron-verify items on already-written fixes. Until they're confirmed on Athena, nothing interactive is trustworthy.

- [~] **AP post-boot scheduling** — APs `loop{hlt}` post-boot; service threads pinned to BSP via `spawn_on_bsp`. MasterChecklist *POST-LOGIN DESKTOP BRING-UP* §; memory `ap-cores-dont-schedule-postboot`. Needed for sustained multicore daily use.
- [~] **Compositor IF=0 spinlock deadlock** — `lock_compositor()` RAII guard, QEMU-verified, **iron pending**. MasterChecklist *POST-LOGIN DESKTOP BRING-UP* §2.
- [x] **xHCI HCE 100ms-grind fix** — 5ms grace window; Tier-6 11.1s→2.8s, HID arming preserved. memory `xhci-hce-grind-fix`.
- Background: **work-stealing stays OFF** (`scheduler.rs`, intermittent steal-resume race) — do not re-enable for v0.1. MasterChecklist *Latent kernel bugs* §4.8.

**Exit:** one safe-image flash showing desktop composites + live cursor + post-boot threads running.

---

## Wave 1 — Live interaction on iron

### WS1 — Live USB HID (gates every interactive item)
- [ ] **Keyboard typing reaches serial echo** — MasterChecklist **2.7** acceptance line 1.
- [~] **USB HID mouse + boot-protocol robustness** — MasterChecklist **2.1** (`find_hid_keyboard_interrupt` protocol capture, mouse routing). Iron cursor pending.
- [~] **Multi-controller xHCI bring-up** — CODE-COMPLETE, iron-verify pending. MasterChecklist **2.1** (binds all 4 Athena controllers). Likely fixes "stick/keyboard only in some ports."
- [~] **Bare-metal HID doorbell fix** — MasterChecklist **2.1** (`doorbell_target` always returns DCI).
- [ ] **HP keyboard port-2 `GET_DESCRIPTOR StallError`** — open. MasterChecklist *POST-LOGIN DESKTOP BRING-UP* follow-ups.
- [~] **Port-4 LowSpeed device stall after recovery** — MasterChecklist **2.1** real-device hardening.

**Depends on:** Wave 0. **Effort:** S–M, verification-heavy.

### WS2 — Ethernet to usable (parallel, independent of WS1)
- [~] **RTL8125 RX → DHCP Bound** — RX `[x]` iron; *"next flash confirms `[net] post-boot: DHCP BOUND`"*. MasterChecklist **2.2** + *IRON VERIFICATION 2026-06-14*; memory `rtl8125-rx-and-net-poll`.
- [~] **DNS resolve** — `SYS_NET_DNS(264)` wired; iron-gated on the lease. MasterChecklist **10.2**; memory `raenet-sockets-dns`.

**Depends on:** Wave 0. **Effort:** S, one flash to confirm.

---

## Wave 2 — Persistent install + usable shell

### WS3 — Install to internal disk, boot from it
- [~] **Kernel mounts root from real NVMe partition** — install + boot-from-disk **QEMU-verified 5/5**; iron pending. MasterChecklist **3.5** line 482.
- [~] **Second power-on without USB boots to user_init** — the one-shot bare-metal test. MasterChecklist **3.5** line 484.
- [~] **User data boot N → N+1** — MasterChecklist **3.5** line 485.
- [~] **First-run setup persists to RaeFS** — account/password proven; on-disk RaeFS formatted. MasterChecklist **3.5** line 483.
- [~] **Graphical install wizard** (`installer_ui.rs`) — live keyboard on iron pending (← WS1). MasterChecklist **16.1** line 1086.
- **3.9 acceptance** lines 515–517.
- ⚠️ **SAFETY (cardinal rule):** the *only* internal-disk write path. Follow `docs/SAFE_ATHENA_BOOT.md`; every byte routes through `block_io::safe_mode_guard_write`; the real install is a deliberate, explicitly-confirmed non-safe boot. memory `install-spine-code-complete`, `athena-install-path`.

**Depends on:** WS1 (wizard UI) — headless `raeinstaller` path does not. **Effort:** M, verification + one careful real install.

### WS4 — Shell usable with no terminal
- [~] **RaeShell as default shell** (login→desktop, taskbar, start, WM). MasterChecklist **14.1** line 1016.
- [~] **Files browses real RaeFS** + [~] **Settings changes + persists wallpaper** — MasterChecklist **14.4** acceptance line 1044 (*"Files/settings ELF + RaeFS browse still `[ ]`"*) + **14.2** lines 1025–1026.
- [~] Notifications (1019) · Search bar (1021) · App switcher (1020) · tray clock (1018) — polish on iron with live input.

**Depends on:** WS1. **Effort:** M, integration.

---

## Wave 3 — The mountains (START NOW, parallel; gate *full* G1, not v0.1)

### WS5 — Real GPU desktop (Phase 6 / 2.5) — biggest effort + risk
- [~] **AMDGPU userspace driver** (`amdgpud`) — daemon + firmware blobs + VBIOS-from-VFCT done; host-KAT'd. MasterChecklist **6.1** line 726; memory `amdgpu-bringup-host-kat`.
- [~] **AMD GPU via userspace Mesa (LinuxKPI host)** — MasterChecklist **2.5** line 405.
- [ ] **wgpu backend wired/tested** (734) · [ ] **Submit to GPU** (743) · [ ] **Direct scanout** (728).
- [~] **virtio-gpu 2D → repoint compositor scanout off GOP CPU blit** — MasterChecklist **6.1** line 723.
- [ ] **Frame time < 16.6 ms at 1080p on Athena** — MasterChecklist **6.5** line 759.
- Real bring-up remaining: register-level PSP/SMU mailbox on real BAR0/BAR5 + Mesa port.

**Depends on:** nothing in Waves 0–2 (independent). **Effort:** L, highest risk. **Ships as:** Update 1 (userspace, no kernel reflash).

### WS6 — Web browser (Phase 14.2)
- [ ] **Web browser** — MasterChecklist **14.2** line 1030 (*"Chromium via RaeBridge or native?"*). `raeweb` component exists (~4.9k LOC, thin).
- **DECISION LOCKED (2026-06-15): defer to fast-follow Update 4.** v0.1 ships no full browser (web on another machine until then; an optional trivial link-opener is the only v0.1 web surface). The engine choice (Servo-class native vs minimal native web view) is itself deferred to Update-4 scheduling. RaeBridge+Chromium stays off the table (years out). **WS6 is parked** — do not schedule engine work until Update 4.

**Depends on:** networking (WS2) + TLS (10.2) + fonts (`raefont`) + ideally GPU (WS5). **Effort:** L. **Ships as:** Update 4 (userspace app).

---

## Wave 4 — Round out daily use (post-v0.1 updates)

### WS7 — Wi-Fi (Phase 2.2)
- [~] **Wi-Fi via userspace LinuxKPI (Path C)** — host bridge ready (syscalls 127–140); **`iwlwifi` source port pending**. MasterChecklist **2.2** line 372. **Ships as:** Update 2 (userspace).

### WS8 — S3 sleep / wake (Phase 2.4)
- [~] **S3 suspend→memory + resume** (396) · [~] **D-state per device** (398) · [~] **CPU C-states** (395).
- [ ] **Suspend→resume completes without panic** — MasterChecklist **2.7** acceptance line 437.
- Hard on real AMD silicon (device save/restore + GPU/USB re-init). **Ships as:** Update 3 (kernel A/B slot).

### WS9 — OTA update + rollback on iron (Phase 3.6)
- [~] **Two-slot kernel layout** (489) · [~] **`raeupdate` writes inactive slot** (490, signature-gated) · [~] **boot fallback** (492).
- [ ] **Atomic CoW update + one-click rollback verified end-to-end** — needs persisted `RAESLOT.CFG` + a real staged-kernel reboot on Athena. MasterChecklist **3.6** line 493.
- This is the vehicle that delivers Updates 1–3 to *other* users. Dogfood can defer it (reflash instead).

---

## Wave 5 — Release hardening (gate shipping to others, not dogfood)

- [ ] **24-hour soak** — MasterChecklist **4.9**.
- [ ] **BOOT-BENCH < 6000 ms** — currently ~11s on iron. MasterChecklist maintenance line 1186 + Concept §Core 3.
- [ ] **Multi-SKU coverage** — MasterChecklist **Phase 17** (≥2 SKUs for the Ship Gate).
- Re-enable SCHED_GAME for the compositor once EDF runtime-budget throttling lands (currently CFS-normal).

---

## Fast-follow update roadmap (after v0.1 dogfood is live)

| Update | Delivers | Workstream | Vehicle | Why this order |
|---|---|---|---|---|
| **1** | AMD GPU acceleration | WS5 | userspace pkg | biggest perf/battery jump; unblocks HDR/VRR/multi-mon later |
| **2** | Wi-Fi | WS7 | userspace pkg | un-tethers the machine |
| **3** | S3 sleep/wake | WS8 | kernel A/B slot | laptop-grade power behavior |
| **4** | Full browser | WS6 | RaeStore app | closes the last "daily" gap |
| **5** | OTA update+rollback, multi-monitor, HDR/VRR | WS9 + 6.4 | mixed | needed to ship to *other* people |

---

## Immediate next actions (this is the unblocked queue head)

1. **Wave 0 flash** — confirm AP scheduling + compositor lock + HID on one safe-image boot. **← the v0.1 critical path is blocked here; the safe image (compositor IF=0 fix + BSP-pin + xHCI HCE fix) is built and ready to flash.**
2. ~~Lock the browser-strategy decision (WS6)~~ — **DONE 2026-06-15: deferred to Update 4** (v0.1 ships no full browser). WS6 parked until Update-4 scheduling.
3. **WS5 (GPU)** — host-KAT'd bring-up sequence is COMPLETE (37 tests, `GpuOps` `None` for unconfirmed MMIO); the next step is real BAR0/BAR5 register access, which is **iron-gated**. No further pre-iron coding runway here — it advances on a GPU-instrumented iron boot, not the dev box.
4. Then ride Wave 1 → Wave 2 to a dogfood-able v0.1.

> **State as of 2026-06-15:** the near-term plan is now **verification-bound**. Waves 1–2 are written + QEMU-verified and gated on the Wave 0 flash; WS5's pre-iron runway is exhausted (host-KAT complete); WS6 is decided + parked. **The ball is the Wave 0 flash** — it both validates the foundation and produces the fresh iron bootlog needed to attack the remaining open items (boot-time breakdown, DHCP-Bound capture, USB-MSC, codec walk).
