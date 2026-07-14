# RaeBridge — Wine Porting & Improvement Strategy

**Status:** analysis / strategy (2026-06-26). Companion to
[`raebridge.md`](raebridge.md) (vision), [`raebridge-process-model.md`](raebridge-process-model.md)
(one-process-per-exe model), and [`raebridge-real-crt-abi.md`](raebridge-real-crt-abi.md)
(GS-base / mprotect ABI). Live status always defers to `MasterChecklist.md` Phase 11.

**Concept lines served** (`RaeenOS_Concept.md` §Compatibility / §Gaming-First):
> "RaeBridge runs Windows apps on day one. Wine + Proton heritage, **tightly
> integrated** — not a 'subsystem', apps run naturally."
> "DirectX 11/12 → RaeGFX translation at the driver level (DXVK/VKD3D-Proton
> lineage, **but integrated and signed**)."
> "Steam works day one via RaeBridge — non-negotiable; without Steam there is no
> PC gaming OS."

---

## 0. TL;DR — the one thing to internalize

**You are not "porting Wine." You already chose a different, harder, more
Concept-pure path: a from-scratch Rust reimplementation of the Win32 surface that
runs on RaeenOS's own syscalls, with Wine/Proton as a _reference_ rather than a
_dependency_.** That path is already working — a genuine MSVC-compiled `.exe` runs
end-to-end (§2). The remaining work is **not** an architecture decision; it is
three concrete structural gaps (cross-process object broker, GUI, live fault
delivery) plus grinding import-coverage depth.

The real strategic forks left to decide are narrow and named in §4 and §5. Don't
re-litigate the foundation — it's sound.

---

## 1. The honest framing: Wine is 3.5M lines of C

| Codebase | Size | Relevance |
|---|---|---|
| Wine source | ~3.5M C | the surface RaeBridge must *functionally* replace |
| RaeBridge today | ~72k Rust | real, working, ~2% of the functional surface by area |
| RaeBridge 1.0 budget | ~800k Rust | single largest first-party module (`MasterChecklist.md` sizing table) |

Two consequences fall directly out of this ratio:

1. **You cannot hand-write all 3.5M lines of equivalent.** Coverage must be
   *demand-driven* — implement what real binaries actually import, in frequency
   order, and let everything else fail loud at call time (the missing-import
   trampoline already does exactly this — see `exec.rs`).
2. **The expensive subsystems (D3D translation) are where source-reuse vs.
   from-scratch is a live, unresolved decision** (§5). For the plain Win32 surface
   the decision is already made and working: from-scratch Rust.

---

## 2. Where RaeBridge actually is (proven, not aspirational)

This is the part most "how do I port Wine" framings get wrong: **the hard
foundational milestone is already met.** Status ladder: `[x]` = iron-proven,
`[~]` = QEMU/host-proven, `[ ]` = not built.

| Capability | Status | Evidence |
|---|---|---|
| PE32+ loader (headers/imports/sections/reloc) | `[~]` | `pe_loader.rs`; every-boot smoketest |
| 16k Win32 names across **158 DLL surfaces** registered | `[x]` | Phase 11.1; `pe_dll_registry.rs` (11k lines) |
| IAT patch + missing-import fail-loud trampoline | `[~]` | `exec.rs`; guest faults at *call* time, not load |
| **Real MSVC `/MT` `.exe` runs to `main` and exits** | `[~]`→iron-pending | commit `226e315`: 73 imports resolved, CRT → `main` → `ExitProcess(0)`, 0 faults, reproduced 2× |
| **Real external `.exe` from an arbitrary VFS path** | `[~]` | Phase 11.2 (2026-06-26): `Target::Pe { path }` handoff, each exe its own process |
| GS-base (TEB) + W^X mprotect ABI | `[~]` | commits `de4845f`/`78bc921`; `SYS_SET_GS_BASE`=282, `SYS_MPROTECT`=283 |
| SEH x64 table-based unwind (`.pdata`/`.xdata`/`__C_specific_handler`) | `[~]` | `seh.rs`; host-KAT 14/14; **live fault→handler delivery still `[ ]`** |
| DXBC→SPIR-V shader translator (slices 1–2) | `[~]` | `dxbc_spirv.rs`; 6 `fxc` fixtures `spirv-val`-clean; **render is GPU-gated** |
| Registry shim backed by versioned config | `[~]` | `win_registry.rs`; survives reboot, rides RaeFS snapshot/rollback |
| Linux `clone(CLONE_THREAD)` / pthreads (Proton prereq) | `[x]` iron | `[[linux-clone-threads-scoping]]` — was the Steam/Proton hang blocker |

**Takeaway:** the "double-click a Windows binary and its real machine code runs on
RaeenOS" promise is *real for genuine compiler output*. What's missing is breadth
and three structural subsystems — not the core mechanism.

---

## 3. The architecture you already have (and why it's correct)

RaeBridge uses **Wine's in-process model**, which is the right call for RaeenOS:

- **Guest VA == host VA.** The PE is `sys_mmap`-mapped into the launcher process's
  own address space; RaeenOS user pages are executable, so guest code runs
  natively (no emulation, no VM). `extern "win64"` lets rustc speak the Microsoft
  x64 calling convention directly into the IAT.
- **One loader process per `.exe`** (`handoff.rs` + `launcher.rs`): a guest
  `ExitProcess` kills only that child; the parent reaps via `SYS_WAIT`. This is the
  structural "each app is its own RaeenOS process" model, and it's what makes the
  capability sandbox (per-app virtual `C:\` = per-app RaeFS bucket) coherent.

**Do not regress this to a separate "Windows subsystem" VM or a Linux-syscall
shim.** The whole Concept thesis is that the layer is *invisible*. The in-process
model is what delivers that.

---

## 4. Strategic fork #1 — harvest Wine source, or keep writing Rust?

This is the decision that defines the next 12 months. The current trajectory is
**from-scratch Rust** (every shim in `kernel32.rs`, `ntdll.rs`, `user32.rs`, … is
hand-written, not transplanted Wine C). Lay the tradeoffs out honestly:

| | From-scratch Rust (current path) | Harvest Wine C source |
|---|---|---|
| Concept fit | ✅ "single signed runtime", memory-safe, integrated | ⚠️ a C dependency tree inside the signed runtime |
| Edge-case fidelity | ❌ re-derives 35 years of app-specific quirks | ✅ inherits Wine's battle-tested behavior |
| Per-DLL cost | High (write + KAT each function) | Low (port + reroute syscalls) |
| Build complexity | None beyond cargo | Needs `zig cc` hermetic toolchain (Phase 11 `[ ]`) |
| Security surface | Rust safety throughout | C UB inside the trusted runtime |
| Maintenance | You own every line | Track Wine upstream forever |

**Recommendation: a deliberate hybrid, decided per-DLL, not per-project.**

- **Keep from-scratch Rust** for the OS-coupled core: `ntdll` (NT syscalls → RaeenOS
  syscalls), `kernel32` process/thread/sync/heap, the loader, TEB/PEB, SEH. These
  *must* be tightly fused to RaeenOS internals; a Wine port would fight the kernel.
- **Consider harvesting Wine source** for the OS-agnostic, pure-logic, enormous-
  surface DLLs where fidelity matters more than integration: `comctl32`, `gdiplus`,
  `riched20`, `msxml`, `oleaut32` marshaling, font/uniscribe text shaping. These are
  the long tail where re-deriving Wine's quirk handling is pure waste.
- **The gating prerequisite** for *any* harvesting is the `zig cc` hermetic
  cross-compile toolchain (already a Phase 11 `[ ]` item). Until that exists,
  harvesting is blocked and from-scratch Rust is the only available path — which is
  fine, because the OS-coupled core (the part you can't harvest anyway) is the
  current frontier.

---

## 5. Strategic fork #2 — the D3D path (DXVK/VKD3D)

This is **explicitly unresolved in the tree today** and worth surfacing because two
docs point in different directions:

- The **Concept doc** says "integrated and signed" → implies first-party Rust.
- `dxbc_spirv.rs` is exactly that: a **from-scratch Rust** DXBC→SPIR-V translator,
  already `spirv-val`-clean for slices 1–2.
- But the **`MasterChecklist.md` sizing analysis** says: *"Aggressive reuse of
  DXVK/VKD3D **source** is the bet — rewriting them from scratch would push
  RaeBridge alone past 2M LOC,"* and Phase 11 still lists `[ ] DXVK port` /
  `[ ] VKD3D-Proton port`.

These cannot both be the plan. The honest reconciliation:

- **Shader translation** (DXBC/DXIL → SPIR-V) is a *bounded, host-provable, pure-CPU*
  problem with an authoritative oracle (`spirv-val`, `fxc`). The from-scratch Rust
  `dxbc_spirv.rs` is a defensible, Concept-pure choice **for this slice**, and it's
  already proving out. Continue it for SM4/SM5.
- **The D3D runtime** (resource/state/command/pipeline management — the other ~90%
  of DXVK/VKD3D) is where from-scratch would blow the LOC budget past 2M. **This is
  where source reuse is the bet.** Port DXVK/VKD3D's *runtime* via `zig cc`, but feed
  it your own validated SPIR-V where the from-scratch translator is ready.
- **Decision needed from the owner:** ratify "from-scratch shader translator +
  ported DXVK/VKD3D runtime" as the official split, and update Phase 11 + `raebridge.md`
  to say so. Right now an implementer could reasonably build either, and that
  ambiguity will waste a wave.

**Everything D3D is GPU-gated regardless** — it cannot render a triangle until
RaeGFX has a real Vulkan submit path on the amdgpu bring-up. So this fork is a
*design* decision to make now and a *build* decision deferred behind the GPU.

---

## 6. The structural gaps that actually block real apps (ranked)

Coverage depth aside, these three subsystems are what stand between "a console exe
runs" and "real Windows software runs," in priority order:

### 6.1 Cross-process object broker (the "wineserver" gap) — **highest leverage**

**Finding (verified 2026-06-26):** named sync objects are not just per-process —
they are *handle-allocation stubs*. `create_mutex_w` ignores `initial_owner`;
`release_mutex` / `set_event` / `reset_event` set last-error and return `TRUE`
without touching any wait/signal state; the object `name` is stored as a label but
never used for cross-process rendezvous (`kernel32.rs:960+`). A single-threaded app
that never contends survives; anything that *relies on the mutex blocking* or on two
processes sharing a `Global\Name` object breaks.

Wine solves this with the **`wineserver`** broker process (Unix sockets). **Do not
port wineserver.** Build a native `raebridge_server` userspace daemon over RaeenOS
IPC (`SYS_IPC_SEND`/`RECV`) + capabilities that owns:
- the named-object namespace (mutex / event / semaphore / file-mapping / named pipe),
- real wait/signal semantics (`WaitForSingleObject`/`MultipleObjects` that actually block),
- `DuplicateHandle` across processes + handle inheritance,
- (later) the window-message-queue routing in §6.2.

Start with named mutexes + events + real `WaitForSingleObject` — the minimum every
multi-process app and Steam itself needs. **This is a `[interface]` change** (new
syscalls or a well-known capability endpoint) → must go through the architect.

### 6.2 GUI: `user32` + `gdi32` → compositor (Phase D)

`user32.rs` (1647 lines) has bones but no real message pump. Needed:
- `GetMessage`/`PeekMessage`/`DispatchMessage` loop backed by §6.1's per-process queue,
- `HWND` → a RaeShell compositor surface (so Win32 windows get glass/HDR/VRR for free),
- `WM_PAINT` → a `gdi32` raster into that surface (SW raster now; RaeGFX-accelerated later).

This is the single biggest fidelity sink and the best candidate for **harvesting
Wine's `user32`/`gdi32`** (§4) once `zig cc` lands — those two DLLs are the most
mature, most quirk-dense part of Wine.

### 6.3 Live SEH fault delivery

The unwind *engine* is done and KAT'd (`seh.rs`, 14/14). The missing half is
**delivering a real hardware fault into a guest `__except` handler** — kernel signal
plumbing (`RtlDispatchException` + the funclet execution phase). Without it, any app
that relies on `__try/__except` for control flow (lots of them, including the MSVC
CRT's own SEH scaffolding under fault) will terminate instead of recovering. Needs
kernel cooperation → coordinate with raeen-kernel; **HUMAN-GATED** (see §8).

---

## 7. Recommended sequencing

Mapped to what's blocked on what. Items in a phase are independent unless noted.

**Phase A — breadth + the broker (unblocked now, no GPU, no toolchain):**
1. **Import-coverage hit list.** Instrument the missing-import trampoline to log
   every `DLL!name` a real target hits; rank by frequency; fill `winapi_shims` in
   that order (this is Phase 11's `[ ] Phase C: 200 most-imported names` made
   data-driven). Cheapest coverage-per-hour in the project.
2. **`raebridge_server` broker** (§6.1) — named mutex/event/semaphore with *real*
   wait/signal. `[interface]` via architect. Highest structural leverage.
3. **Registry thunk wiring** — `advapi32 RegOpenKeyExW/RegQueryValueExW` → the
   already-built `win_registry.rs` versioned-config shim (the backing store is done;
   only the PE-facing thunks are missing).

**Phase B — real fault handling (needs kernel work, human-gated):**
4. Live SEH dispatch (§6.3) + kernel signal plumbing.

**Phase C — GUI (designable now; SW-rasters now, GPU-accelerates later):**
5. Message pump over the broker's per-process queue.
6. `CreateWindowEx` → RaeShell compositor surface; `WM_PAINT` → `gdi32` SW raster.
7. **Acceptance:** `notepad.exe` runs, types, saves (Phase 11.3 gate #1).

**Phase D — games (gated on RaeGFX real-GPU submit):**
8. Continue `dxbc_spirv.rs` (SM4/SM5: control flow, textures, integer/compute stages).
9. Ratify + build the DXVK/VKD3D-runtime source port (§5) behind `zig cc`.
10. Vulkan surface from RaeGFX → D3D runtime → **Steam runs** (the non-negotiable test).
    Threads are no longer the blocker — `CLONE_THREAD`/pthreads landed on iron
    (`[[linux-clone-threads-scoping]]`).

---

## 8. Process constraints (do not skip)

- **HUMAN-GATED.** Net-new RaeBridge *guest-execution* work requires explicit owner
  assignment (standing policy; the 2026-06-22 wave was a scoped lift, not a blanket
  one). Design, host-KATs, ABI-surface proposals, and the broker's *interface* are
  fair game to prepare; landing new guest-execution paths is not, without a go.
- **ABI changes go through the architect.** The broker (§6.1) and any new SEH/signal
  syscalls are `[interface]` commits: `rae_abi` number + dispatch arm + `docs/SYSCALL_TABLE.md`
  in one `RAEEN_AGENT=opus`-tagged commit. Bump `ABI_VERSION` on any break.
- **No Linux clones.** RaeBridge speaks *native* RaeenOS syscalls; it is not a Linux
  personality. The guest is Windows code; the host substrate is RaeenOS, not a POSIX
  emulation. (Linux-ELF support is a *separate* track in `linux_syscall.rs`.)
- **Every guest-visible primitive is capability-gated and sandboxed** — a Windows app
  sees a virtual `C:\` that is its own RaeFS bucket; no global registry, no shared
  state, per the Concept sandbox model.

---

## 9. Anti-patterns (each one wastes a wave)

1. **"Just bolt on the DXVK/Wine binaries as `.so`s."** Breaks "single signed
   runtime", drags in a Linux userland, and contradicts the Concept. The bet is
   *source reuse compiled into the signed runtime*, not binary bolt-on.
2. **Porting wineserver.** Use native RaeenOS IPC + capabilities (§6.1).
3. **Implementing all 16k names before running anything.** The trampoline lets real
   apps run with partial coverage; let *data* (the hit list) drive the order.
4. **Re-deriving `comctl32`/text-shaping from scratch** once `zig cc` exists — that's
   the long tail where Wine harvesting earns its keep (§4).
5. **Claiming D3D progress without `spirv-val`.** The translator's only honest proof
   is the validator exit code; rendering is GPU-gated and cannot be faked green.

---

## 10. Open decisions for the owner

1. **Ratify the D3D split** (§5): from-scratch Rust shader translator + ported
   DXVK/VKD3D *runtime*. Update Phase 11 + `raebridge.md` to remove the ambiguity.
2. **Approve the `raebridge_server` broker** as the next RaeBridge `[interface]`
   slice (§6.1) — it's the highest-leverage unblocked structural item.
3. **Green-light the `zig cc` hermetic toolchain** (Phase 11 `[ ]`) — the gate that
   unlocks *all* Wine/DXVK/Mesa source reuse; nothing in §4/§5's harvesting half can
   start without it.
4. **Wine upstreaming policy** (carried over from `raebridge.md`): strict downstream
   harvest vs. contribute-back. Affects how §4 harvesting is structured.

---

## Provenance

Built from a direct read of `components/raebridge/` (72,569 lines across 39 modules)
on 2026-06-26: `exec.rs`, `kernel32.rs:960+` (named-object stub finding), `ntdll.rs`,
`d3d_translate.rs`, `dxbc_spirv.rs`, `handoff.rs`, `launcher.rs`; the Phase 11/12 and
sizing sections of `MasterChecklist.md`; and the existing `docs/components/raebridge*.md`.
Related memory: `[[raebridge-real-exe-execution]]`, `[[raebridge-runs-real-windows-exe]]`,
`[[raebridge-seh-engine]]`, `[[linux-clone-threads-scoping]]`, `[[anticheat-two-tier-strategy]]`.
