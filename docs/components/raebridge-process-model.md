# Spec: AthBridge guest-process isolation + double-click `.exe` launch

## Concept promise served

> "**AthBridge runs Windows apps on day one.** Wine + Proton heritage, tightly
> integrated. Not a 'subsystem' — apps run naturally." (LEGACY_GAMING_CONCEPT.md §"The
> hardest problem isn't the OS" / switching plan, line 221)

Reinforced by §Compatibility (line 117): "**Steam works day one** via AthBridge".
Steam *spawns* child processes (the storefront launches each game as its own
process) — so "apps run naturally" is structurally impossible until each `.exe`
is its own OS process. This spec maps the architecture from "AthBridge executes a
PE *in-host*" to "double-click a Windows app and it runs as its own sandboxed
AthenaOS process."

---

## Already in the tree (verify-before-implement)

The execution machinery is **built and verifier-proven** — this spec designs only
the *process-model delta*, not the loader/CRT work that already runs.

- **PE load + execute, in-host** — `components/raebridge/src/exec.rs`
  `load_pe_executable()`: mmap RW → copy/reloc → IAT patch → mprotect `.text` RX
  → build TEB/PEB → `set_gs_base` → caller jumps the entry. `[~]` (commit
  `226e315`; raeen-verifier independent PASS, reproduced 2×, 0 faults). A genuine
  VS2022 `cl.exe` `/MT` C exe AND a C++ static-ctor exe run to `main`, print, and
  `ExitProcess`. This is the load path the launcher reuses **unchanged**.
- **The in-host harness (the thing this spec replaces)** —
  `raebridge_host/src/main.rs`: a `_start` that loads FIVE bundled fixtures in
  ONE process and runs the C++ fixture *as the process terminator*. Its own
  docstring states the constraint: "Only ONE real CRT exe can run to its
  ExitProcess per host process (it terminates us)." That is the structural wall.
- **AthBridge ABI primitives** — `SYS_SET_GS_BASE`=282, `SYS_MPROTECT`=283 (+
  `PROT_*`) landed in kernel + `rae_abi`; scheduler saves/restores `Task::gs_base`
  at all 3 switch sites. `[~]` (commits `de4845f`, `78bc921`). `docs/SYSCALL_TABLE.md`
  Block 33. **Next free syscall number: 293** (284–290 were taken by anti-cheat
  attestation Block 34, and 291–292 by the surface-resize Block 35, *after* the
  first draft of this doc recommended 284 — corrected 2026-06-29 to avoid the
  §10-pitfall-#1 collision class).
- **Native ELF spawn by osabi byte** — `kernel/src/syscall.rs` arm 11 (`SYS_SPAWN`):
  reads the path from VFS, routes by `elf_loader::detect_elf_origin` —
  `ELFOSABI_ATHENAOS` (0xAE) → `scheduler::spawn_elf_task_with_pty`; Linux ELF →
  `linux_exec`. **`rdx` already carries the optional PTY slave id** (NOT free for
  argv). `[x]` (the spawn-and-reap chain boots).
- **The argv gap (confirmed)** — `kernel/src/task.rs`: `new_elf_with_pty` (native,
  line 579) sets up the user stack with **no argc/argv** (just rbp/rsp); ONLY the
  Linux path `new_elf_linux` (line ~688) calls `elf_loader::setup_linux_stack`
  with argv/envp/auxv. So a native AthenaOS process today receives **no arguments**.
  This is the load-bearing ABI gap.
- **The launcher's home** — `user_init/src/main.rs` `sys_spawn(path: &[u8])` (line
  82) wraps `SYS_SPAWN` with **only `rdi`/`rsi`** (no third arg passed). user_init
  spawns `raebridge_host` unconditionally (line 565). This is where a launcher gets
  spawned and where the demo-host harness retires to a fixture-only role.
- **App-launch path** — `kernel/src/shell_runner.rs` `spawn_app_from_vfs(path)`
  (line 2870): resolves via `app_paths::resolve_candidates` → `vfs::read_file` →
  `scheduler::spawn_elf_task(&elf_data, None)` → `rae_manifest::assign_for_spawn`
  for the sandbox level. Apps registered via `add_app` carry an `exec_path`. `[~]`
  (Start→Enter→spawn→surface live-proven by the beta-tester at `f530a67`).
- **File-type detection** — `components/rae_formats/src/lib.rs`:
  `detect(bytes) -> FileKind` returns `FileKind::Pe` on the `MZ` magic (line 450),
  and `Pe.category() == Category::Executable` (line 266). `detect_with_hint`
  (content-over-extension). **Already used by Files' Quick Look** (commit `ed4fc8a`).
  Double-click PE detection needs **zero new format code**.
- **Per-app data buckets** — `kernel/src/data_buckets.rs`: `on_task_spawn(task_id)`
  creates an isolated per-task bucket; `parse_path` / `can_access` / `read` /
  `write` enforce ownership; `reclaim_task_resources` calls `on_task_exit`. AthFS
  buckets + `Cap::Filesystem{root_inode}` scoping are `[x]` iron-proven (cross-app
  read rejected — MasterChecklist line 1236). This is the substrate the virtual
  `C:\` binds to.

**Net:** the loader, CRT surface, GS/mprotect ABI, sandbox buckets, and file-type
detection all exist. The delta is: (1) a launcher ELF that runs ONE PE per process,
(2) the argv/target-passing ABI so the launcher learns *which* PE, (3) the Files /
shell double-click wiring, (4) binding the bucket as the guest `C:\`.

---

## Prior art & OSS verdict

- **Wine `wine64`/`wineloader`** — one Unix process per Windows process; the loader
  binary (`wine`) is `exec`'d with the target `.exe` path as `argv[1]`, builds the
  PEB/TEB, maps the PE, jumps the entry. `CreateProcess` → a fresh `wine` exec.
  *Verdict:* **📖 study/isolate** (LGPL-2.1). AthBridge already carries Wine
  *lineage* (Concept §line 45 "Wine/Proton lineage, isolated"); we mirror the
  **one-loader-process-per-exe pattern**, not the code. The launcher = our
  `wineloader`.
- **Proton / Steam** — the storefront process spawns each game as a child
  (`CreateProcess`/`SteamLaunch`). Confirms the requirement: Steam *cannot* work
  until guest `CreateProcess` lands a real OS process. *Verdict:* in-use lineage;
  GPU-gated, out of scope for this spec.
- **WSL `init` / `/init`** — Linux pico-process model: a launcher stub receives a
  target + argv over a well-known channel. Pattern reference for the no-ABI
  alternative (option b). *Verdict:* 📖 conceptual only.
- **POSIX `posix_spawn` / Redox `exec`** — argv lands on the new task's stack
  (argc, `char**`, envp). AthenaOS already does this on the **Linux** spawn path
  (`setup_linux_stack`); we extend the convention to a **native** AthenaOS stack
  layout. *Verdict:* in-use (our own `elf_loader::setup_linux_stack`); reuse the
  stack-building technique, native ABI.

No new external dependency is vendored. The launcher is pure AthBridge + raekit.

---

## Design

### 1. The launcher model — `raebridge-run`

A new **bundled native-osabi ELF** (`ELFOSABI_ATHENAOS` 0xAE), crate
`raebridge_run/` (sibling of `raebridge_host/`), that:

1. Reads its target PE path (or bundled fixture name) from its arguments — see §2.
2. `vfs::read_file`s (or `SYS_OPEN`+`SYS_READ`s) the PE bytes into a heap buffer.
3. Calls the **existing** `raebridge::exec::load_pe_executable(&bytes)` — the same
   mmap RW → reloc → IAT → mprotect RX → TEB/PEB → `set_gs_base` path that the host
   harness already exercises. (No loader changes; it is process-agnostic by
   construction — it only touches *this task's* address space.)
4. Installs the process-global Win32 session (`FullCompatSession::new` +
   `install_host_context`, exactly as `raebridge_host` does) seeded with this PE,
   `enable`s whichever milestone arm is appropriate (none, for production launches).
5. **Jumps the entry as `unsafe extern "win64" fn() -> !`** and lets the guest run
   to its own `ExitProcess`.

The critical inversion: in the host harness, a guest `ExitProcess`/`sys_exit`
**terminates the whole host** (so only the *last* fixture may ExitProcess). In the
launcher model, `raebridge-run` **IS the process** — its `sys_exit(code)`
terminates only *this* child. The parent (Files / AthShell / Steam-equivalent)
reaps the code via `SYS_WAIT` (13), exactly like any native app. One `.exe` =
one `raebridge-run` process = one OS-reapable exit code.

```
  Today (in-host):                  This spec (per-process):
  ┌─────────────────────┐          ┌──────────────┐ ┌──────────────┐
  │ raebridge_host      │          │ raebridge-run│ │ raebridge-run│
  │  ├ fixture A (ret)  │          │  └ app1.exe  │ │  └ app2.exe  │
  │  ├ fixture B (ret)  │          │   ExitProc → │ │   ExitProc → │
  │  └ fixture C        │          │   sys_exit   │ │   sys_exit   │
  │     ExitProc KILLS  │          └──────┬───────┘ └──────┬───────┘
  │     the host        │                 │ reaped         │ reaped
  └─────────────────────┘          ┌──────┴────────────────┴───────┐
   1 exe/process MAX                │ parent (Files/shell) SYS_WAIT │
                                    └────────────────────────────────┘
                                     N exes = N isolated processes
```

**Buildable NOW (no kernel/ABI dependency):** the launcher crate skeleton, the
`load_pe_executable` call, the session install, the entry jump. It is structurally
identical to `raebridge_host::_start` minus the multi-fixture loop — only the
*argument plumbing* (§2) is ABI-gated.

`raebridge_host` is **not deleted** — it is demoted to the **fixture smoketest
harness** (its current durable boot proof of the loader/CRT, run unconditionally),
while `raebridge-run` becomes the production launch path. Two crates, one loader
(rule 7: extend the WIRED loader, don't fork it — both call `exec.rs`).

### 2. The argv / target-passing ABI gap — RECOMMENDATION: (a) `sys_spawn_args` = **293**

The launcher must learn WHICH PE to run. Two options were specced; **(a) is
recommended** because it is the general primitive every real shell/Steam needs
(argv is not AthBridge-specific), it is the smallest durable surface, and it
reuses the proven `setup_linux_stack` stack-building technique on the native path.

#### Option (a) — RECOMMENDED: new `SYS_SPAWN_ARGS` = syscall **293**

`SYS_SPAWN` (11) cannot be widened: `rdx` is already the PTY id, and overloading
the remaining registers with an args pointer would be a one-off magic-number widen
(violates the interface-steward "never widen casually" rule). Add a **dedicated**
syscall instead.

```
nr  | name              | rdi      | rsi      | rdx      | r10        | r8       | rax
293 | SYS_SPAWN_ARGS    | path_ptr | path_len | argv_ptr | argv_count | pty_id   | child pid / err
```

- `argv_ptr` points at a **packed argv blob** in the caller's address space:
  `argv_count` records, each `[u32 len][u8 bytes...]` (no NUL, length-prefixed,
  back-to-back). The kernel `copy_from_user`s the whole blob (bounded:
  `SPAWN_ARGS_MAX_BYTES` = 64 KiB, `SPAWN_ARGS_MAX_COUNT` = 64) and validates with
  `validate_user_range` — no raw deref (matches net/capture hardening).
- `pty_id` moves to **r8** (0 = none), freeing the semantics to be a strict
  superset of `SYS_SPAWN` (so `SYS_SPAWN` becomes `SYS_SPAWN_ARGS` with
  `argv_count = 0`).
- Routing is **identical** to arm 11 (osabi byte): native → a new
  `scheduler::spawn_elf_task_with_args`; Linux → `linux_exec(name, argv)` (which
  *already* takes an argv slice — the Linux path needs no new kernel work, just the
  wired argv).

**Where args land in the child (the AthenaOS native convention).** Today
`new_elf_with_pty` builds a bare native stack. The new
`Task::new_elf_with_args` writes a native argv block onto the child's user stack
*before* entry, using the **same descending-stack technique** as
`elf_loader::setup_linux_stack` but with the **AthenaOS native ABI** (NOT the Linux
auxv layout — rule 2, no Linux-clone). Proposed native convention, documented in
`docs/SYSCALL_TABLE.md` Block 36 (the launcher's own block; 34 is anti-cheat,
35 is surface-resize):

```
  high ┌────────────────────────┐  NATIVE_USER_STACK_TOP
       │ arg strings (UTF-8,    │
       │ NUL-terminated, packed)│
       │ ...                    │
       │ [16-byte align pad]    │
       │ argv[argc] = NULL      │  ┐
       │ argv[argc-1] ptr       │  │ pointer array (into the strings above)
       │ ...                    │  │
       │ argv[0] ptr            │  ┘
  rsp→ │ argc (u64)             │  ← entry sees argc at [rsp], argv at [rsp+8]
       └────────────────────────┘
```

The native runtime entry stub (`raekit::_start` / `raebridge_run::_start`) reads
`argc` at `[rsp]` and `argv` at `[rsp+8]` (AthenaOS native, deliberately *unlike*
the Linux `setup_linux_stack` because native ELFs are not Linux-ABI). raekit gains
`raekit::args() -> ArgsIter` reading this block; `raebridge-run` reads `argv[1]`
as the target PE path.

**rae_abi additions (NEEDS-INTERFACE — see below):**
`syscall::SYS_SPAWN_ARGS = 293`, `SPAWN_ARGS_MAX_BYTES`, `SPAWN_ARGS_MAX_COUNT`,
the packed-record reader/writer helpers (`pack_argv`/`decode_argv`, host-KAT'd).
**No `ABI_VERSION` bump** (a fresh number breaks no existing signature — same
posture as 279/280/281).

**Capability:** inherits SYS_SPAWN's policy — if the caller holds any
`Cap::Process`, at least one must include `EXEC` or `E_RIGHTS`; legacy permissive
otherwise (bring-up). The launched child's *sandbox* is set by the spawner via the
existing `rae_manifest::assign_for_spawn` path (§4), not by this syscall.

#### Option (b) — no-ABI alternative (fallback if 293 is blocked)

The launcher reads its target from a **per-spawn VFS handoff**: the spawner writes
the target path to a well-known per-child path
`/run/raebridge/<child-pid-or-token>.target` *before* spawning, and `raebridge-run`
reads `/run/raebridge/self.target` (a per-task VFS alias resolving to the spawner's
written file) at startup. No syscall change.

*Why it's the fallback, not the pick:* (1) it needs a per-task VFS view or a
race-free token rendezvous (the spawner must write *before* the child reads, and a
PID can be reused), (2) it does nothing for Steam's generic `CreateProcess` argv
needs, (3) it leaks launch intent into the FS namespace. It is the right move
**only** if the spawn path stays hot and 293 cannot land for a release — it
unblocks double-click with zero kernel risk by piggybacking the proven VFS path.
The decision below assumes (a) ships once the spawn path cools.

#### ⚠️ Sequencing gate (HOT FILE)

The kernel spawn path is **currently contaminated by concurrent work** — committed
`scheduler.rs` carries an SMP=2 deadline-starvation regression (MasterChecklist
line 91) and `main.rs`/`scheduler.rs` are actively dirty under the concurrent GPU +
sched_proof sessions. **The 293 kernel impl (touching `scheduler.rs`/`task.rs`
spawn) MUST NOT land until those files cool**, exactly as the GS-base impl was
gated on `scheduler.rs` cooling (line 54). Until then: build the launcher crate +
the `rae_abi` constant + the host-KAT'd `pack/decode_argv` (all off the hot files),
and wire double-click against option (b) if a release is needed sooner.

### 3. The double-click flow, end-to-end

```
 User double-clicks app.exe in Files          (apps/files — raeen-shell-apps)
        │
        │ rae_formats::detect(bytes) == FileKind::Pe   (EXISTS, commit ed4fc8a)
        ▼
 Files maps "open executable" → launch request
        │  target = "/path/app.exe"
        ▼
 SYS_SPAWN_ARGS(293):  path="raebridge-run", argv=["raebridge-run","/path/app.exe"]
        │              (option a)   — OR — write /run/raebridge handoff + SYS_SPAWN (option b)
        ▼
 kernel: read raebridge-run ELF (osabi 0xAE) → spawn_elf_task_with_args
        │  child gets native argv stack (§2) + per-task bucket (data_buckets::on_task_spawn)
        │  rae_manifest::assign_for_spawn("app.exe", child_pid) → sandbox level
        ▼
 raebridge-run::_start  reads argv[1] → load_pe_executable → install session →
        │               jump entry → unmodified Win32 code runs
        ▼
 Win32 window: guest calls user32 CreateWindow → AthBridge shim → SYS_SURFACE_CREATE(24)
        │  → compositor surface (SHARED input path: SYS_POLL_MOUSE/READ_KEY/INPUT_CURSOR)
        ▼
 guest ExitProcess(code) → shim → sys_exit(code) → ONLY this child dies
        ▼
 parent SYS_WAIT(13) reaps code → Files shows it like any closed app
```

**Shell-launcher reuse.** The existing tile path (`shell_runner::spawn_app_from_vfs`
+ `add_app{exec_path}`) extends to PE targets with one branch: when the resolved
target is a PE (or the `exec_path` is tagged `raebridge:<name>`),
`spawn_app_from_vfs` spawns **`raebridge-run` with the PE path as argv** instead of
spawning the PE directly (a native ELF spawn of an `MZ` file would be rejected by
`elf_loader` — PE is not ELF). A Windows app can thus be a Start-menu tile: its
`AppEntry.exec_path = "raebridge-run /apps/win/app.exe"` and the launch path splits
on the space into path + argv. This is the **same** `spawn_app_from_vfs` →
`assign_for_spawn` → `PENDING_TITLES` flow that the 18 native apps already use; the
only new code is the PE-target branch.

**Window through the compositor / shared input.** Nothing new: the guest's Win32
window is a normal `SYS_SURFACE_CREATE` surface (the AthBridge user32 shim already
targets the compositor), so it composites, focuses, and receives input through the
exact same path as native apps. Per-process isolation does not change the
compositor contract — it *enables* it (today the in-host harness has no window; a
real app does).

### 4. Per-process sandbox — the virtual `C:\` ⇄ per-app bucket binding

Concept §"Security by default": each guest process is sandboxed by default.

- **Binding.** On spawn, `data_buckets::on_task_spawn(child_pid)` already creates an
  isolated per-task AthFS bucket (`[x]` iron-proven cross-app isolation). The
  AthBridge file shims (`kernel32 CreateFileW`/`ReadFile`/`WriteFile`, already
  implemented) translate the guest's `C:\` namespace onto **this child's bucket
  root**: a guest path `C:\Users\...\foo.txt` maps to bucket-relative
  `Users/.../foo.txt` resolved via `data_buckets::{read,write}` / `open_in_bucket`,
  so app A cannot see app B's `C:\` (they are different buckets). `C:\Windows`
  system DLLs resolve to a **read-only shared system bucket** (the bundled
  AthBridge DLL set), never the per-app writable bucket.
- **Cap gate (coordinate with raeen-security — DO NOT design AthGuard internals
  here).** File and net access from a guest must pass a capability. The proposed
  cap surface to hand raeen-security:
  - A guest process holds `Cap::Filesystem{root_inode = its bucket root}` only —
    so the existing `check_bucket_cap` rejects any out-of-bucket resolve (already
    the mechanism for native apps).
  - Net access (`ws2_32`/`winsock2` shims) routes through a **`Cap::Net`-gated**
    path; a guest without the cap gets `WSAEACCES`-equivalent, fail-closed.
  - The sandbox *level* (Trusted / AppSandbox / Untrusted) comes from the app's
    `RaeManifest.toml` via the **existing** `rae_manifest::assign_for_spawn`
    (called in `spawn_app_from_vfs` today) — AthBridge apps get a manifest entry
    like any bundle. **Naming + the exact cap variants are raeen-security's call;**
    this spec only states the *binding points* (bucket = `C:\`, manifest =
    sandbox level, `Cap::{Filesystem,Net}` = the gates).
- **Reclaim.** `reclaim_task_resources` already calls `data_buckets::on_task_exit`
  + sweeps sockets/sandbox entry on *any* exit (self or kill), so a crashed or
  killed guest leaks neither its bucket handle nor its sockets — the per-process
  model inherits the proven reclaim path for free.

### Failure modes

- **Guest crashes / faults** → only `raebridge-run` dies; parent reaps a non-zero
  code (e.g. the loader's `0xDEAD` missing-import exit, or a fault-derived code).
  The desktop survives ("driver crash ≠ system crash" generalized to apps).
- **PE fails to load** (not 64-bit, bad reloc, too large) → `raebridge-run` exits
  with a named code and prints the `exec.rs` `ExecError` to serial; parent surfaces
  "couldn't open app".
- **Missing import** → existing fail-loud stub: `[raebridge] FATAL: call into
  unresolved import: dll!name` then `sys_exit(0xDEAD)` — now isolated to the one
  child, not the host.
- **argv blob malformed / oversized** → `SYS_SPAWN_ARGS` returns `E_INVAL`; no
  child spawned (validated before the stack is built).
- **PID reuse race (option b only)** → mitigated by a per-spawn token, not bare PID
  — another reason (a) is preferred.

### Security model

W^X is preserved per-process (`exec.rs` mprotect RX flip runs in each child). Each
child's GS base points at *its own* TEB. The argv blob is copied + bounds-checked
in-kernel (no TOCTOU on user memory). The bucket cap fails closed once
raeen-security wires enforcement. No guest gains authority over another guest's
address space (separate page tables — the whole point of the per-process move).

---

## Interface needs (NEEDS-INTERFACE → raeen-architect)

1. **`SYS_SPAWN_ARGS = 293`** (next free per `docs/SYSCALL_TABLE.md`; reserved
   there 2026-06-29). Signature:
   `rdi=path_ptr, rsi=path_len, rdx=argv_ptr, r10=argv_count, r8=pty_id` →
   `rax=child pid / err`. Packed argv: `argv_count` × `[u32 len][bytes]`. Bounds:
   `SPAWN_ARGS_MAX_BYTES=65536`, `SPAWN_ARGS_MAX_COUNT=64`. Cap: inherits SYS_SPAWN
   (`Cap::Process{EXEC}` when held). **No `ABI_VERSION` bump** (fresh number).
   Add the row to `docs/SYSCALL_TABLE.md` (new Block 36) in the same `[interface]`
   commit; promote the reserved 293 row to live and bump "next free" to 294.
2. **`rae_abi` constants + helpers:** `SYS_SPAWN_ARGS`, the two bounds, and
   `pack_argv(&[&str]) -> Vec<u8>` / `decode_argv(&[u8], count) -> Vec<&str>`
   (host-KAT'd round-trip, FAIL-demonstrated on a truncated record). These let the
   spawner and the kernel agree on the wire format without magic numbers.
3. **Native argv stack convention** documented in `docs/SYSCALL_TABLE.md` Block 36
   (the `argc@[rsp]`, `argv@[rsp+8]`, NULL-terminated `char**` layout) — explicitly
   marked AthenaOS-native (NOT Linux auxv).

If 293 is deferred behind the hot spawn path, the architect signs off **option
(b)** as the interim (no ABI change) and 293 lands when `scheduler.rs` cools.

---

## File-by-file plan

- **`rae_abi/src/lib.rs`** (raeen-architect, `[interface]` commit): add
  `syscall::SYS_SPAWN_ARGS = 293`, bounds consts, `pack_argv`/`decode_argv` +
  host KATs.
- **`docs/SYSCALL_TABLE.md`** (same commit): Block 36 row + native argv layout +
  reserved-range update.
- **`raebridge_run/`** (NEW crate — raeen-compat): `src/main.rs` `_start` reads
  `argv[1]`, `vfs`/`SYS_OPEN`-reads the PE, `load_pe_executable`, installs the
  session, jumps the entry; `Cargo.toml` mirrors `raebridge_host` (raekit
  allocator + raebridge). **Buildable now** against option (b); swaps to `argc@rsp`
  argv when 293 lands.
- **`raebridge_host/src/main.rs`** (raeen-compat): demote to fixture-only smoketest
  harness (keep the 5 durable FAIL-able fixtures + verdict lines — the loader's
  boot proof); add a one-line docstring noting `raebridge-run` is the production
  per-process launcher.
- **`kernel/src/syscall.rs`** (raeen-architect, GATED on spawn-path cool): arm 293
  → `copy_from_user` argv blob → `decode_argv` → route by osabi to
  `spawn_elf_task_with_args` / `linux_exec(name, argv)`.
- **`kernel/src/scheduler.rs` + `kernel/src/task.rs`** (raeen-architect, GATED):
  `spawn_elf_task_with_args` + `Task::new_elf_with_args` building the native argv
  stack (technique from `elf_loader::setup_linux_stack`, native layout).
- **`components/raekit/`** (raeen-compat): `raekit::args()` reading `argc@[rsp]`;
  update `_start` to preserve the stack arg block.
- **`user_init/src/main.rs`** (raeen-compat): keep spawning `raebridge_host` as the
  fixture harness; (later) spawn `raebridge-run` for the smoketest-2 proof (§proof).
- **`kernel/src/shell_runner.rs`** (raeen-shell-apps): in `spawn_app_from_vfs`,
  branch PE targets / `raebridge:<name>` exec_paths to spawn `raebridge-run` with
  the PE path as argv.
- **`apps/files/`** (raeen-shell-apps): on double-click of a `FileKind::Pe`, issue
  the launch (SYS_SPAWN_ARGS or option-b handoff) instead of Quick Look.
- **AthBridge file shims** (raeen-compat): translate guest `C:\` → this task's
  `data_buckets` root; `C:\Windows` → read-only shared system bucket.

---

## Acceptance criteria (the exact proof)

The FAIL-able isolation proof — **two real exes as SEPARATE processes**, each runs
to its own `ExitProcess`, both exit codes reaped by the parent:

- Boot log MUST show, in order:
  - `[raebridge-run] launched pid=<A> target=<exeA> -> running` and
    `[raebridge-run] launched pid=<B> target=<exeB> -> running`
  - `[raebridge] guest ExitProcess(<a>) -> exit <a>` from process A AND
    `[raebridge] guest ExitProcess(<b>) -> exit <b>` from process B
  - the parent: `[isolation] reaped pidA=<A> code=<a> pidB=<B> code=<b>
    distinct_pids=true both_reaped=true -> PASS` (FAILs if either child is unreaped,
    if the two share a PID, or if the first ExitProcess killed the second — the
    exact regression this spec exists to prevent).
- Pick the two existing fixtures with DIFFERENT exit codes: the
  `ExitProcess(42)` fixture (→ 42) and the C++ `ExitProcess(0)` fixture (→ 0) —
  `42 != 0` proves the codes are real, not a fixed sentinel. (Today the host can
  only run ONE to ExitProcess; running BOTH to completion IS the proof.)
- `/proc/raeen/raebridge` (or `procfs`) MUST report `launched_total`,
  `live_processes`, and `last_exit_code` per launch (the per-process surface the
  in-host model can't have).
- The host KAT `pack_argv`/`decode_argv` round-trip MUST pass + FAIL on a truncated
  record (`cargo test -p rae_abi`).
- The launcher's docstring MUST quote the Concept promise above (R10).
- No new `[boot] WARN`; no panic; `System successfully booted` present.

---

## Handoff

- **Implementers:**
  - **raeen-compat** — `raebridge_run` crate (the launcher), `raebridge_host`
    demotion to fixture harness, raekit `args()`, the `C:\`→bucket file-shim
    translation, the two-exe isolation smoketest.
  - **raeen-architect** — `SYS_SPAWN_ARGS=293` in `rae_abi` + `SYSCALL_TABLE.md`
    (`[interface]` commit), and the kernel spawn-path impl (`syscall.rs` arm,
    `scheduler.rs`/`task.rs` native argv stack) **GATED on the spawn files
    cooling**.
  - **raeen-shell-apps** — `shell_runner::spawn_app_from_vfs` PE-target branch +
    `apps/files` double-click→launch wiring.
  - **raeen-security** — name + wire the guest `Cap::{Filesystem(bucket-scoped),
    Net}` gates and the AthBridge `RaeManifest.toml` sandbox-level entry (this spec
    states the binding points; AthGuard internals are theirs).
- **Unblocks checklist lines:** Phase 11 "AthBridge runs Windows apps"
  (MasterChecklist §1428); the AthBridge-wave "double-click a .exe and it runs"
  milestone (lines 53/58) graduates from in-host to per-process; line 1455
  ("games run via AthBridge + Steam") is downstream of guest `CreateProcess`, which
  this enables.
- **Sequencing:**
  1. **Now (off hot files):** `raebridge_run` crate skeleton + `load_pe_executable`
     call + session install; `rae_abi` 293 constant + `pack/decode_argv` host KAT;
     option-(b) interim wiring for double-click if a release is needed.
  2. **`[interface]` commit:** 293 + `SYSCALL_TABLE.md` (architect, RAEEN_AGENT=opus).
  3. **When `scheduler.rs`/`task.rs` cool (SMP=2 starvation + sched_proof + GPU
     work landed):** the kernel spawn-path impl as ONE SMP-verified slice (≥5 boots
     SMP=1 and =2 — the SMP/timing rule).
  4. Files/shell double-click flips from option (b) to 293; the two-exe isolation
     smoketest goes green.

---

## Open questions for the lead

1. **Crate name:** `raebridge_run` (Cargo) bundled as `raebridge-run`? Confirm the
   initramfs manifest name the shell will reference.
2. **`C:\Windows` system bucket:** is there an existing read-only shared bucket for
   bundled DLLs, or does this need a new shared-bucket primitive (coordinate with
   raeen-fs)?
3. **Option (a) vs (b) for the *first* shippable double-click:** is the spawn path
   expected to cool soon enough to wait for 293, or should double-click ship on (b)
   first and migrate? (Affects whether raeen-shell-apps wires (b) at all.)

---

# Spec: Cross-process sync broker — the fsync-parity fast path (§6.1, Slice 2b)

> Companion to the process-isolation spec above. Once N Windows processes are
> isolated (§1–§4), they must rendezvous on named sync objects (`Global\Name`
> mutex/event/semaphore). This section specs the **performance contract** of that
> rendezvous — the invariants that keep it at Linux-Wine-with-fsync level instead
> of regressing to the wineserver round-trip that esync/fsync exist to kill.

## Concept promise served

> "**Gaming isn't a mode. It's the default.**" + "**Steam works day one** via
> AthBridge — non-negotiable." (LEGACY_GAMING_CONCEPT.md §Core Principles / §Compatibility)

Steam and every non-trivial multi-threaded Windows app rely on `WaitForSingleObject`
*actually blocking* and on two processes sharing a `Global\Name` object. The
performance of that path is the one place in AthBridge where the "faster than Wine"
question is decided by **design**, not tuning — because the other big cost (D3D→Vulkan
translation) is shared with Wine via DXVK/VKD3D source reuse (`raebridge-wine-strategy.md`
§5). The sync path is where AthBridge can match fsync — or silently undercut it.

## Background: why fsync beats wineserver (the cost being avoided)

Classic Wine routed *every* sync op through the `wineserver` broker process over a
Unix socket — a context switch per `WaitForSingleObject`/`ReleaseMutex`, even
uncontended. esync moved this to `eventfd`; **fsync** moved it to a raw futex word
in shared memory, so the common case (no contention) is a single userspace atomic
CAS — zero syscalls, zero broker IPC, zero context switches. fsync was a large,
measurable gaming win precisely because it removed the broker from the hot path.

**AthBridge must replicate fsync's structure, not wineserver's.** The danger is
subtle: a broker daemon is the right home for the *namespace and lifetime*, but if
it ends up on the *per-operation* path, AthBridge reinvents the exact tax it should
be beating.

## Already in the tree (verify-before-implement)

The fast-path scaffolding is **built and FAIL-able-self-tested**; Slice 2b is the
live blocking wiring on top of it.

- **Namespace (Slice 1)** — `components/raebridge/src/broker.rs` `BrokerNamespace`:
  `create`/`open`/`close`/`page_id` resolve `Global\Name` + kind → a stable
  **shared-page id** with refcounting and kind-collision rejection.
  `run_namespace_self_test()` (boot self-test #9, `raebridge_boot.rs:333`) is `[~]`.
- **Shared-page state machine (Slice 2a)** — `broker.rs` `SharedSyncState`: the
  futex word at offset 0 *is* the object state (`broker.rs:264`), with the atomic
  fast paths already written and proven:
  - `event_set`/`event_reset`/`event_try_wait` (auto-reset CAS 1→0; manual-reset load),
  - `mutex_try_acquire`/`mutex_release` (CAS 0→tid; owner-recursion via `recursion`),
  - `sem_try_acquire`/`sem_release` (CAS decrement/bounded increment).
  `run_shared_state_self_test()` (boot self-test #10, `raebridge_boot.rs:344`) is `[~]`.
- **Kernel futex** — native `SYS_FUTEX = 258` (`syscall.rs:3297`, "native futex for
  relibc sync"; distinct from the Linux-ABI `SYS_FUTEX = 202`). Wait/wake queue in
  `locking.rs:2139`/`2153`. AthBridge guests are native-osabi (0xAE) → they use 258.
- **In-process named-object stubs (the thing being replaced)** — `kernel32.rs:1050+`
  `create_mutex_w` & friends + `finish_create_sync`/`CreateSyncResult`. The §6.1
  finding stands: `release_mutex`/`set_event` currently set last-error and return
  `TRUE` without touching wait/signal state; the `name` is a label, never a
  cross-process rendezvous. Slice 2b rewires these onto the broker page + futex.

**Net:** the namespace, the shared-page encoding, and the atomic transitions all
exist and self-test. Slice 2b is (1) mapping the page cross-process, (2) the
userspace wait/wake loop around the existing atomics, (3) two kernel correctness
fixes below, and (4) the wake-elision refinement.

## The fast-path contract (the invariants Slice 2b must hold)

### Invariant 1 — uncontended op = zero syscalls, zero broker IPC

The broker daemon is consulted **only** at `Create`/`Open`/`Close` (cold, once per
handle). It is **never** consulted on `Wait`/`Signal`/`Release`. Those run entirely
against the mapped shared page via the existing `SharedSyncState` atomics. If any
wait/signal path issues a `SYS_IPC_SEND` to `raebridge_server`, the contract is
broken — that is the wineserver regression in disguise.

| Path | Owner | Cost |
|---|---|---|
| `CreateMutex`/`OpenEvent`/`Close` (namespace + lifetime) | broker daemon (IPC) | cold |
| `Wait`/`Release`/`SetEvent`, uncontended | `SharedSyncState` atomic on the page | **0 syscalls** |
| `Wait` that must block / signal that must wake a parked waiter | `SYS_FUTEX` (258) on the page word | slow edge only |

### Invariant 2 — the futex key is the shared-page identity, never a virtual address

Two processes map the same object at **different** VAs. The kernel futex bucket
**must** be keyed by the shared-page identity (shmem object id + offset), the way
Linux keys a *shared* futex by `inode+offset` — not by the caller's VA. If keyed by
VA, `Global\Foo` in process A and process B hash to different buckets, so a wake in
A never reaches a waiter in B → every cross-process block hangs forever.
`BrokerNamespace::page_id` (`broker.rs:166`) already yields that stable id; the
wait/wake calls must pass *it* (page-id-relative), not the mapped pointer.
**This is the single highest-risk correctness item in the broker.**

### Invariant 3 — `futex_wait` must re-check `*addr == expected` atomically under the wait-queue lock

`locking.rs:2139` currently discards `expected` (`let _ = expected;`, line 2149): it
enqueues unconditionally. That is the classic **lost-wakeup race** — waiter reads the
word, decides to block; signaler changes the word + wakes *before* the waiter
enqueues; the waiter then enqueues and sleeps forever. A correct futex re-reads
`*addr` **while holding the bucket lock** and returns `EAGAIN` (don't sleep) if it no
longer equals `expected`. Live-vs-dead: the path is **live** (relibc pthreads use it)
but the bug is **latent** — the window is narrow at `-smp 1`, exactly the
"passed twice, hangs on the third boot" class (CLAUDE.md rule 17). It must be fixed
before the broker can be trusted under real cross-process contention, and it is a
kernel change → pairs with the `[interface]` work and gets the ≥5-boot SMP=1/=2 gate.

### Invariant 4 — elide the wake when no waiter is parked

`SharedSyncState::event_set`/`mutex_release` today return a wake count
unconditionally (`broker.rs:319`/`369`) — correct for *state*, but in Slice 2b a
naive caller would then always issue `futex_wake`, putting a syscall on the
uncontended signal path. fsync avoids this with a waiter count. **Add a second word**
to `SharedSyncState` — `waiters: AtomicU32` — incremented right before `futex_wait`
and decremented on wake/timeout; the signal path issues `futex_wake` **only if
`waiters > 0`**. Without this, AthBridge beats wineserver but loses to fsync on the
uncontended signal — the precise regression this spec exists to prevent.

### The canonical wait loop (around the existing atomics)

```
WaitForSingleObject(obj):
    loop:
        if state.try_wait():          # event_try_wait / mutex_try_acquire / sem_try_acquire
            return WAIT_OBJECT_0       # uncontended → returned with ZERO syscalls
        state.waiters.fetch_add(1)     # publish intent BEFORE re-checking (no lost wakeup)
        if state.try_wait():           # re-check after publishing — closes the race
            state.waiters.fetch_sub(1); return WAIT_OBJECT_0
        SYS_FUTEX(WAIT, page_word, expected=unsignaled)   # Invariant 3 makes this race-free
        state.waiters.fetch_sub(1)
```

The signal side (`SetEvent`/`ReleaseMutex`/`ReleaseSemaphore`) runs the existing
`SharedSyncState` transition, then — and only if `waiters > 0` — `SYS_FUTEX(WAKE, n)`
with `n` from the transition (1 for auto-reset / mutex, `WAKE_ALL` for manual-reset,
`n` for a semaphore release).

## Regression modes (each defeats the perf comparison)

- **Broker on the hot path** → reinstates the wineserver round-trip (Invariant 1).
- **VA-keyed futex** → cross-process blocks hang (Invariant 2).
- **`expected` discarded in `futex_wait`** → intermittent lost-wakeup hangs (Invariant 3).
- **Unconditional `futex_wake`** → a syscall on every uncontended signal; slower than
  fsync (Invariant 4).
- **Namespace lock held across a wait** → a blocked waiter inside the broker would
  freeze the whole namespace; the broker lock is touched only on create/open/close.
- **Thundering herd on a manual-reset event** → intended (`WAKE_ALL`); but an
  *auto-reset* event must wake exactly one (`n = 1`) or it spuriously runs extra
  waiters that immediately re-block — wasted wakeups.

## Interface needs (NEEDS-INTERFACE → raeen-architect)

1. **Cross-process shared-page mapping primitive.** The broker page must be mappable
   into each participating guest's address space by a stable id. **Verify first**
   whether an existing shmem/channel-map syscall covers this (the Slice-2 boot note
   `raebridge_boot.rs:331` references a `SYS_CHANNEL_SHMEM_MAP`-style primitive — it
   is **not** in `syscall_table.rs` today, so confirm vs. design). If none exists,
   it is a new `[interface]` syscall: map a broker-owned page by `page_id` →
   guest-local VA, capability-gated (a guest maps only pages for objects it has a
   handle to). Number from the reserved range, `SYSCALL_TABLE.md` row + dispatch arm
   in one `RAEEN_AGENT=opus` commit; bump `ABI_VERSION` only if a signature changes.
2. **Native `SYS_FUTEX` (258) wired for the broker key space + the `expected`-compare
   fix** (Invariant 3) in `locking.rs`. No new number — same syscall, corrected
   semantics. Kernel change → the ≥5-boot SMP=1/=2 gate (CLAUDE.md rule 17).
3. **`SharedSyncState` layout addition** — `waiters: AtomicU32` (Invariant 4) is a
   shared-page wire-format field; freeze it with `version`/`SHARED_SYNC_VERSION`
   (already present, `broker.rs:270`) so a layout bump is detectable across the
   broker↔guest boundary.

## Acceptance criteria (the proof that makes "fsync-parity" measurable, not vibes)

Per CLAUDE.md rule 8, a perf claim needs a counter. `/proc/raeen/raebridge` (or the
broker's procfs line) MUST expose, and the smoketest MUST assert:

- **`uncontended_op_syscalls` == 0** — N uncontended acquire/release/set/wait ops
  cause **zero** `SYS_FUTEX` calls and **zero** broker IPCs. Nonzero = a hot-path
  syscall leak (Invariant 1/4 violated). This is the headline FAIL-able number.
- **`broker_ipc_per_sync_op` == 0** on wait/signal (nonzero only on create/open/close).
- **Cross-process rendezvous PASS** — a FAIL-able boot/host test: process A blocks on
  `Global\E`, process B `SetEvent`s it, A wakes with `WAIT_OBJECT_0`. Must hang-detect
  (bounded wait → FAIL, not deadlock) so Invariant 2/3 regressions print FAIL rather
  than freezing the boot.
- **Contended correctness** — auto-reset event wakes exactly one of two waiters;
  manual-reset wakes both; mutex hand-off preserves ownership; semaphore count never
  exceeds `max_count`. (Extends the existing `run_shared_state_self_test` from
  single-thread transitions to real two-waiter blocking.)
- The broker module's R10 docstring quotes the Concept promise above; no new
  `[boot] WARN`; `System successfully booted` present.

## Handoff

- **raeen-compat** — Slice 2b: rewire `kernel32.rs:1050+` named-object shims onto
  `BrokerNamespace` + `SharedSyncState` (replace the return-TRUE stubs), implement the
  wait loop + wake-elision (`waiters` word), and the two-process rendezvous smoketest.
- **raeen-architect** — the shared-page mapping primitive (`[interface]`) + the
  `futex_wait` `expected`-compare fix in `locking.rs` (Invariant 3), as one
  SMP-verified kernel slice.
- **raeen-debugger** — owns confirming Invariant 3 is the lost-wakeup class on the
  native 258 path (reproduce at SMP=2) before/after the fix.
- **Sequencing:** the shared-page primitive + `futex_wait` fix gate the live blocking;
  the namespace and atomics (Slices 1/2a) are already green, so the userspace wait
  loop + wake-elision can be host-KAT'd now and wired once the kernel half lands. This
  is **HUMAN-GATED guest-execution work** (`raebridge-wine-strategy.md` §8) — design,
  host-KATs, and the `[interface]` proposal are fair game; landing the live guest
  wait/wake needs explicit owner go.
- **Unblocks:** `raebridge-wine-strategy.md` §6.1 (the highest-leverage structural
  gap), and every multi-process / multi-threaded-contended Windows app — Steam
  included.
