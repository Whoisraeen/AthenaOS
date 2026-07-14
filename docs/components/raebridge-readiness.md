# AthBridge Readiness Report

**Status as of 2026-06-30.** Ceiling is `[~]` (host KAT + QEMU); no iron flashes on
this path. This is the consolidated answer to "how close is AthBridge to *a real
Windows GUI app runs, types, saves, windows, exits — with cross-process sync +
exception handling*", and what remains for the GPU-gated Phase D.

---

## 2026-06-30 update — API Set redirection + import-binding fast path

Two in-crate, no-ABI slices grounded in real Windows 10.0.26200 ground truth
(gathered on this dev box with VS2022 `dumpbin`; reproducible tooling in
`components/raebridge/tools/`, survey output to `$env:TEMP\raeen-winapi`):

- **API Set schema redirection (`src/apiset.rs`) — the level-of-detail win.** A
  survey of 500 real System32 `.exe` import tables showed the ~40 most-imported
  symbols are imported through **API Set contract DLLs** (`api-ms-win-core-*`,
  `api-ms-win-crt-*`), not `kernel32.dll`. AthBridge's exact-name resolver never
  matched them, so every modern (non-hand-crafted) binary's imports fell through
  to fail-loud stubs. Now an authoritative `api-set -> host` table — generated
  from this machine's 111 `System32\downlevel` forwarder stubs and folded onto
  the AthBridge module that hosts each export — redirects contract DLLs the way
  the real Windows loader (and Wine) do. Wired into `winapi_shims::resolve_shim`,
  `pe_loader::DllRegistry::{resolve,resolve_ordinal,has_dll}`, and
  `WinApiModule::from_name`.
- **`ShimResolver` (build-once, O(log n)) — the speed/efficiency win.** The old
  `resolve_shim` rebuilt the whole multi-thousand-entry shim table (a `Vec`
  alloc + every fn-pointer cast) on *every* call, once per imported symbol.
  `ShimResolver` builds the sorted table once per image load and binary-searches
  it; `exec.rs` uses it for the import loop. Native equivalent of the dynamic
  linker's hashed symbol table, with no per-lookup allocation.

- **Coverage-hit-list batch #1 — 25 real shims (`winapi_shims.rs`).** New
  `tools/analyze-coverage.ps1` ranks the still-missing survey imports by
  real-binary frequency (`missing-ranked.txt`); the top tractable batch is now
  real behavior, not fail-loud stubs: msvcrt `memset/memcpy/memmove/memcmp/
  wcschr/_wcsicmp` (the dynamic `/MD` CRT imports these), kernel32 SRW locks
  (in-process, uncontended-exact — cross-thread blocking is broker-gated),
  `LocalAlloc/LocalFree`, `OutputDebugStringW`, `DebugBreak`,
  `HeapSetInformation`, `GetModuleFileNameA`, `CreateMutexExW/CreateSemaphoreExW`,
  and advapi32 ETW `EventRegister/Unregister/WriteTransfer/SetInformation`
  (no-session success no-op). All route through the API Set redirect.
- **DXBC→SPIR-V: SM5 integer min/max + imad + bit-manipulation
  (`dxbc_spirv.rs`).** 10 more opcodes while the GPU path matures:
  `imin/imax/umin/umax` (GLSL S/UMin/Max), `imad` (IMul+IAdd), `bfrev`
  (OpBitReverse), `countbits` (OpBitCount), `firstbit_lo/hi/shi`
  (FindILsb/FindUMsb/FindSMsb, with the D3D `31 - pos` MSB flip + -1 sentinel).
  fxc fixture `intbit_ps.dxbc` + `spirv-val`-clean KAT.
- **`raebridge_server` broker sign-off packet** (`raebridge-server-design.md` §7):
  versioned `AthSyncObject` shared-page struct + the one binary ABI decision
  (Outcome A: zero ABI vs Outcome B: `SYS_RAEBRIDGE_SYNC_OPEN=259`) filed as
  `NEEDS-INTERFACE` for Opus.

**Proof:** 208 host lib KATs + 32 dxbc KATs (0 fail), incl. FAIL-able
`api_set_imports_resolve_to_the_same_shim_as_the_host_dll`,
`shim_resolver_matches_resolve_shim_and_builds_once`, the coverage-batch
mem/str/SRW/ETW KATs, and `dxbc_spirv_int_minmax_bit_ps` (spirv-val exit 0).
Full `cargo run -p xtask --release -- build --release` exit 0 (both images).
Ceiling stays `[~]` — no iron flash on this path.

Authoritative status lives in `MasterChecklist.md` Phase 11; this file is the
single-page snapshot the campaign goal asks for at the GPU seam.

---

## The 6-item DONE gate

| # | Item | Status | Evidence |
|---|------|--------|----------|
| 1 | notepad-class GUI .exe runs/types/SW-renders/saves-to-bucket/reaped | **`[~]` MET** | `gui-notepad` smoketest (below); "reaped" satisfied by the #2 process-isolation proof |
| 2 | two .exes as separate processes w/ distinct reaped codes | **`[~]` MET** | QEMU `process-isolation` smoketest: `raebridge_run` pid 91 (`exit42`→42) + pid 92 (`cpp`→0), distinct PIDs + codes, reaped — via **option (b)** (existing `SYS_SPAWN` + `SYS_WAIT4`=61, NO scheduler.rs) |
| 3 | cross-process named sync (A blocks `Global\E`, B SetEvents, A wakes), `uncontended_op_syscalls==0` | **host half `[~]`, kernel half gated** | `broker.rs`/`sync_engine.rs` Slice 2b host done (`uncontended_op_syscalls=0` live in `/proc/raeen/raebridge_syncbroker`); needs physical-futex re-key + real blocking |
| 4 | guest `__try/__except` recovers a real fault | **engine `[~]`, delivery gated** | `seh.rs` host-KAT'd; needs live fault→handler kernel signal plumbing |
| 5 | `dxbc_spirv` SM4/SM5 spirv-val-clean | **`[~]` MET** | dxbc KATs 31/0, spirv-val forced |
| 6 | Phase 11 `[~]`, Phase D `[ ]` | **as stated** | items 1/2/5 `[~]`; D left `[ ]` (GPU-gated, OUT) |

**4 of 6 met (`[~]`): #1, #2, #5, #6.** Only the **#3 kernel half** and **#4** remain,
and both genuinely require touching the SMP=2-fragile `scheduler.rs` / fault-signal
path the owner parked (breadth-over-scheduler-risk); their host/off-hot-file halves
are built and KAT'd, ready to land the kernel slice when authorized. **CORRECTION
(2026-06-29):** an earlier draft of this report listed #2 as kernel-gated on
`SYS_SPAWN_ARGS=284`; that conflated the gate (two reaped processes — achieved NOW
via option (b)) with the future ergonomic argv ABI (option a, `SYS_SPAWN_ARGS`,
re-numbered to **293** since 284 was taken by anti-cheat). The gate item is MET;
option (a) is a later convenience for shell/argv spawning, not a gate requirement.

---

## What's proven `[~]` (host KAT + QEMU)

### A real Notepad-class `.exe` (guest machine code, in QEMU)
One cl.exe Win32 `.exe` (`fixtures/gui_notepad.c`, 18 imports all IAT-wired,
`dumpbin`-verified) registers a window + WndProc, creates a system **EDIT** child,
builds a **File menu** (Save/Exit), **types** "HI" into the EDIT via the standard
message pump, then a **menu-driven File→Save** reads the EDIT with `GetWindowTextW`,
picks a path with `GetSaveFileNameW`, and `WriteFile`s to `C:\note.txt` — landing in
a **per-app-isolated bucket** (both `CreateFileW` and `NtCreateFile` route through
`CompatContext::win_path_to_vfs` → `/mnt/win_c/<app-bucket>/…`).

Underlying pieces, each individually KAT'd: window-text APIs, the built-in EDIT
control (system-class create + WM_CHAR accumulation + WM_PAINT raster), comdlg32
Open/Save dialogs, menus (`CreateMenu`/`AppendMenuW`/`SetMenu` → WM_COMMAND), and
the per-app `C:\` bucket isolation.

### dxbc → SPIR-V translator — the universal SM4/5 surface
First-party Rust (`dxbc_spirv.rs`), validated by `spirv-val` (the only translator
proof). Covers: ALU + all conversions (ftoi/itof/ftou/utof) + float/int comparisons
+ movc + transcendentals (exp/log/sincos, sqrt/rsq/rcp/frc/round) + dp2/3/4 +
structured if/else + loops/break + texture sampling (Texture2D, Texture2DArray,
TextureCube, implicit + explicit LOD) + **discard** (clip/alpha-test → OpKill) +
**ddx/ddy** (→ OpDPdx/OpDPdy) + **constant buffers** (`cb<slot>[n]` → uniform block).
Validated end-to-end with the **canonical vertex shader** (`mul(pos, mvp)` from a
cbuffer → SV_Position) and a real pixel shader — i.e. realistic shaders, not just
isolated opcodes.

---

## Serial markers (QEMU boot, 0 real PANIC, `System successfully booted`)

```
[raebridge] smoketest: gui-window exe -> RegisterClass+CreateWindow+UpdateWindow->WM_PAINT painted 2048 px (WndProc dispatch + reentrant gdi OK) PASS
[raebridge] smoketest: gui-save exe -> typed 'HI' + CreateFileW/WriteFile -> C:\out.txt readback 'HI' PASS
[raebridge] smoketest: edit-control -> CreateWindowEx("EDIT") + typed 'HI' via pump -> GetWindowTextW 'HI' + WM_PAINT rendered PASS
[raebridge] smoketest: gui-notepad exe -> window+EDIT+menu File->Save (GetSaveFileNameW) -> C:\note.txt 'HI' PASS
[raebridge_run] launched pid=91 target=bundled:exit42 -> running   (then guest ExitProcess(42) -> exit 42 PASS)
[raebridge_run] launched pid=92 target=bundled:cpp -> running
[raebridge] smoketest: process-isolation -> 2 PIDs (childA exit=42, childB exit=0) reaped PASS   (item #2)
[raebridge] smoketest: real external .exe via VFS (pe:/home/raeen/rae-app.exe) -> executed + exit 0 PASS
[raebridge] cross-process sync engine (fast-path/wake-elision) self-test -> PASS   (/proc/raeen/raebridge_syncbroker: uncontended_op_syscalls = 0)
```

## KAT tallies
- `cargo test -p raebridge --lib`: **195 / 0** (incl. `sync_engine` 5: deterministic + two real-thread rendezvous + bounded-timeout)
- `RAEEN_SPIRV_VAL=1 cargo test -p raebridge --test dxbc_spirv_kat`: **31 / 0** (spirv-val forced)

---

## What's kernel-gated (parked for explicit owner authorization)

The two remaining gate items require changes to the hot scheduler / fault-signal
path, which carries a committed SMP=2 starvation regression (MasterChecklist line ~91)
that makes the required ≥5-boot SMP=2 proof unreliable. (Item #2 is NOT here — it is
met via option (b) on the existing spawn/reap syscalls; see the correction above.)

- **#3 cross-process sync kernel half** — re-key `SYS_FUTEX` (258) by shared-frame
  *physical* identity (today it is VA-keyed) + a real blocking `futex_wait` (today a
  single cooperative `yield_task`). The userspace driver + accounting (`sync_engine.rs`)
  consume this via the `FutexOps` trait — the kernel half supplies the real impl.
- **#4 live SEH delivery** — `RtlDispatchException`/funclet fault→guest-handler
  kernel signal plumbing (the engine + unwind are done + host-KAT'd in `seh.rs`).

A future ergonomic (NOT a gate item): `SYS_SPAWN_ARGS=293` (reserved in
`SYSCALL_TABLE.md` Block 36) for argv-based spawning by a shell/Steam — its kernel
impl is also scheduler.rs-gated, but the per-process isolation gate it would serve is
already met via option (b).

**Recommendation:** land **#3 first, behind a `FUTEX_PRIVATE`-style flag** so the
physical re-key + real block are isolated from the SMP=2-fragile general futex path.
That lets the change be landed and verified (≥5 boots, SMP=1 and =2) without risking
the known regression, and closes gate item #3 honestly.

---

## What's left for Phase D — the D3D-runtime ↔ AthGFX-submit seam (GPU-gated, OUT)

The shader **translator** is done and spirv-val-clean (item #5). The open seam is the
D3D **runtime** ↔ **AthGFX submit**:

1. **D3D runtime** (resource/state/command-list/pipeline/present — ~90% of DXVK/VKD3D):
   ratified to be a *source port* of DXVK/VKD3D via `zig cc`, not rewritten
   (`raebridge-wine-strategy.md` §5). It consumes the translator's SPIR-V and issues
   draw/dispatch.
2. **AthGFX submit**: the runtime's `vkQueueSubmit`-equivalent must reach AthGFX's real
   GPU submit path — which is gated on the amdgpu bring-up (the MES `set_hw_resources`
   ack, per the amdgpu campaign). No host proof exists past `spirv-val`; real pixels
   need real silicon.

Until the GPU submit path lands, D3D rendering (and Steam-with-graphics) cannot be
proven here, and per scope must **not** be faked.

---

## Remaining non-gated, non-GPU work (lower-leverage)

The universal shader surface and the host-provable GUI are complete. What's left here
is genuinely the niche/meatier frontier, in rough value order:
- dynamic (register-indexed) constant-buffer access (`cb0[r0.x]` — skinning/instancing;
  needs relative-addressing decode in the core operand decoder)
- `gather4` (PCF shadows), multisample textures, typed UAV load/store, compute stages,
  bitfield ops — all real but progressively less universal, UAV/compute GPU-gated
- import-coverage depth beyond the current fixtures (corpus-limited)
- minor GUI polish (multiline EDIT layout, comdlg32 A-variants, TrackPopupMenu)
