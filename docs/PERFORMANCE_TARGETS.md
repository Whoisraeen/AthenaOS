# AthenaOS Performance Targets

The embodiment-first thesis made measurable. AthenaOS's reason to exist is **latency**:
windows-class breadth with console-class responsiveness. This doc is the single
authoritative budget — every number here is a contract a subsystem must hold, with how
it's measured and where it stands today. Scattered perf claims elsewhere defer to this.

**Status legend:** ✅ meets target (proven) · 🟡 partial / QEMU-only · ⬜ not yet measured.
**Iron vs QEMU:** QEMU TCG inflates latency ~3–4×; targets are **bare-metal** numbers.
A QEMU figure is a smoke signal, not a pass.

---

## 1. Boot

| Metric | Target | Stretch | Measured how | Status |
|---|---|---|---|---|
| Kernel T0 → userspace | < 2.0 s | < 1.0 s | `[BOOT-BENCH] T0->userspace` (TSC) | 🟡 ~1.9 s TCG / ~0.7 s iron |
| Full boot → desktop-ready | < 6.0 s | < 3.0 s | `[boot]` marker timestamp; `[boot] WARN` if > 6 s | 🟡 ~6.25 s TCG (network-dominated) |
| Resume from S3 | < 1.0 s | < 0.5 s | wall-clock to first frame | ⬜ S3 not built (Phase 2.4) |

**Live gate:** `[BOOT-BENCH]` + the `[boot] WARN` are machine-enforced every CI boot —
this is the boot-time guard that replaced the LOC budget. The current TCG overage is
**userspace network bring-up** (`vnet_probe ~1064ms` + `dhcp ~936ms`), not kernel mass;
backgrounding those is the open win.

## 2. Frame / compositor (the "feels instant" axis)

| Metric | Target | Stretch | Measured how | Status |
|---|---|---|---|---|
| Compositor frame deadline @60 Hz | 16.6 ms | — | per-frame VRR pacer budget | 🟡 |
| @120 Hz | 8.3 ms | — | " | 🟡 |
| @144 Hz / @240 Hz | 6.9 / 4.2 ms | — | " | 🟡 |
| Missed-frame rate (steady state) | < 0.1 % | 0 | `/proc/athena/perf` `frame.miss_rate_bp` (frametime > 1.5× refresh budget = skipped vsync) | 🟡 counter wired into the present path + logic-proven; live data desktop/iron-gated |
| **GPU-scanout present pipeline @1080p** | ≥ 120 fps (frame ≤ 8.3 ms) | ≥ 240 fps | `[compositor] present-bench(gpu-scanout)` at DCN attach + `/proc/athena/compositor` `present: frame_us/blit_us/fps` (row-blit + clflush present; pacer target 8 333 µs on attach) | 🟡 instrument + fast path landed 2026-07-02; iron number pending next DCN boot. Panel >60 Hz photons additionally need the DCN modeset (Phase 2.3 EDID). |
| Input → photon (added by OS) | < 1 frame | < 4 ms | hardware capture (iron) | ⬜ |
| **VRR**: present within refresh window | always | — | pacer + scanout timestamp | 🟡 VRR pacer exists |

**Rule:** the compositor runs on **SCHED_BODY** (above SCHED_FIFO). A game's frame, the
compositor's composite, and the present must all land inside the refresh window — no
compositor-induced latency. Glassmorphism/blur must stay within the frame budget or be
dropped, never blow the deadline.

## 3. Audio (AthAudio)

| Metric | Target | Stretch | Measured how | Status |
|---|---|---|---|---|
| Round-trip latency (in→process→out) | < 3 ms | < 1.5 ms | loopback timestamp | 🟡 design target; HDA PCM not on iron |
| Buffer underruns under game load | 0 | 0 | `/proc/athena/perf` `audio_underruns` / `audio_periods` (live xrun counter; `record_audio_period(wrote==0)` at audio.rs) | 🟡 counter wired + live; 0-under-game-load needs HDA PCM on iron |
| Audio thread wake jitter | < 100 µs | < 50 µs | `/proc/athena/perf` `audio_jitter.{avg,max}_us` (\|realized period − budget\| at record_audio_period) | 🟡 wired + logic-proven; live needs sustained audio (iron, Phase 7) |

**Rule:** the audio mix thread runs on **SCHED_BODY**; its deadline is hard. Sub-3ms is
the headline AthAudio promise — measured end-to-end once HDA PCM playback lands (Phase 7).

## 4. Input

| Metric | Target | Stretch | Measured how | Status |
|---|---|---|---|---|
| USB HID poll rate | 1000 Hz | 1000 Hz | xHCI interrupt interval | 🟡 |
| Gamepad (DualSense/Xbox) report → event | < 1 ms | < 0.5 ms | input pipeline trace | 🟡 parsers exist |
| Keypress → event delivered | < 1 ms | — | i8042/HID IRQ → input.rs | 🟡 |
| Input event → game (wake latency) | < 1 ms | < 0.5 ms | `/proc/athena/perf` `input_game_wake.{avg,max,last}_us` (input IRQ → next SCHED_BODY dispatch) | 🟡 pipeline wired + logic-proven; live µs desktop/iron-gated |

**Rule:** input is IRQ-driven and the consuming game thread is SCHED_BODY — an input
event must preempt normal work. No batching that adds latency on the game path.

## 5. Scheduler (the engine under all of the above)

| Metric | Target | Stretch | Measured how | Status |
|---|---|---|---|---|
| Context-switch cost | < 2 µs | < 1 µs | TSC around switch_context; decision/scan now `/proc/athena/perf` `sched_pick.{min,avg,max}_ns` | 🟡 pick/scan cost wired (live); the asm register/stack swap itself still ⬜ (needs an asm probe or ping-pong harness) |
| SCHED_BODY wakeup latency (IRQ→run) | < 10 µs | < 5 µs | input-driven case: `/proc/athena/perf` `input_game_wake.*_us` (input IRQ → SCHED_BODY dispatch, §4) | 🟡 input-driven wake measured; non-input (timer/IPC) wakes not separately traced |
| EDF deadline-miss rate (SCHED_BODY) | 0 | 0 | per-class miss counter | 🟡 EDF exists |
| Timer tick / preemption granularity | ≤ 1 ms | ≤ 250 µs | LAPIC timer cfg | ✅ |

**Rule:** SCHED_BODY is a **hard real-time class above SCHED_FIFO** — games, compositor,
audio. Its deadlines are not best-effort. Work-stealing stays off until the steal-resume
race is fixed (intermittent rsp=0 #DF — see scheduler memory).

## 6. Storage (AthFS / NVMe)

| Metric | Target | Stretch | Measured how | Status |
|---|---|---|---|---|
| NVMe 4K random read latency | < 100 µs | — | submit→completion TSC | ⬜ iron |
| AthFS CoW write amplification | < 1.3× | < 1.1× | bytes-written / logical | ⬜ |
| Snapshot create | < 50 ms | — | freeze bitmaps+inode table | 🟡 works, untimed |
| Game-asset streaming throughput | ≥ NVMe line rate | — | sustained read bench | ⬜ |

## 7. Memory

| Metric | Target | Stretch | Measured how | Status |
|---|---|---|---|---|
| Heap alloc (slab fast path) | < 500 ns | < 200 ns | `perf::measure_heap_alloc` TSC microbench → `/proc/athena/perf` `heap_alloc.{min,avg,max}_ns` | 🟡 ~129 ns avg / 69 ns min TCG (already < target; iron pending) |
| Page fault → resident | < 5 µs | — | fault handler trace | ⬜ |
| Hugepage-backed game heap | available | — | mmap flag honored | 🟡 hugepages exist |

---

## 8. How we measure (the telemetry surface is now wired)

- **`/proc/athena/perf` is the one place**, lock-free per-metric counters, fed by the
  hot-path `record_*` helpers. As of 2026-06-25 it carries: boot time; input events +
  input→photon latency; frames presented + frametime ring + FPS + **missed-frame rate**
  (§2); audio periods + **underruns** (§3); SCHED_BODY dispatches + **deadline-miss rate**
  + lateness (§5); context-switch count + per-class picks + runq depth; **scheduler
  pick/scan latency** (§5); **heap-alloc fast-path min/avg/max ns** (§7); **input→game
  wake latency** (§4/§5). Each has a FAIL-able boot smoketest proving the recording logic.
- **What's measured vs. iron-gated:** the *logic* of every wired metric is proven in QEMU
  CI (FAIL-able smoketests). The *live numbers* for frame/input/audio/storage need a real
  workload (the compositor presenting to a panel, real keystrokes, HDA PCM, NVMe I/O) and
  so are **iron-gated** — that is their correct terminal state here, not a gap.
- **Iron is the only real pass.** Frame/input/audio latency cannot be trusted under TCG
  (it inflates ~3–4×); these flip to ✅ only with hardware capture on Athena.

### Iron-gated, awaiting Athena flash (code-complete + QEMU-green)
- §1 full-boot < 6 s: QEMU 13.5 s is dominated by the **TCG-only xHCI HCE wedge**
  (~3.5 s, absent on iron) + module smoketests; the iron 11.1 s root-cause needs the
  `[tier*-prof]` buckets read on a real Athena boot.
- §2 missed-frame rate, §4 input→game wake, §5 input-driven SCHED_BODY wakeup,
  input→photon: live µs/rate need the compositor + real input on a panel.
- §3 audio round-trip + 0-underruns + wake jitter: need HDA PCM on iron (Phase 7).
- §5 heap-alloc < 500 ns / pick-scan < 2 µs: QEMU shows ~130 ns / ~1.5 µs (already under
  budget pre-deflation); the iron pass is a hardware `/proc/athena/perf` read.
- §6 NVMe latency / CoW write-amp: need real NVMe I/O on iron.

### Not applicable by current design
- §7 **page-fault → resident**: AthenaOS **eager-maps**; the fault handler kills the task
  (user fault) or panics (kernel fault) — there is no demand-paging "fault → resident"
  path to time. Revisit only if lazy paging is ever added.

## 9. Next steps (the remaining un-wired telemetry)

1. **Full context-switch cost (§5 asm swap half)** — `switch_context` only "returns" when
   the task is rescheduled, so the register/stack swap needs an asm rdtsc probe (per-CPU
   scratch surviving the stack swap) or a ping-pong microbench. The decision/scan half is
   already wired (`sched_pick`). Treat as a ≥5-boot scheduler change.
2. **Audio thread wake jitter (§3)** — variance of consecutive audio-period wakes vs the
   2.67 ms period, recorded at the `record_audio_period` site.
3. **Boot `[boot] WARN`** — the iron 11.1 s is the one machine-enforced gate we miss;
   root-cause from the per-tier buckets on the next Athena boot (the QEMU number is
   inflated by a non-iron artifact).

> The discipline: a perf claim is `[~]` until there's a counter behind it, and `[x]` only
> with an iron measurement. Same ladder as everything else (see `TESTING_STRATEGY.md`).
