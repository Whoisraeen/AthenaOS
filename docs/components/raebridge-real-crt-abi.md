# RaeBridge real-MSVC-CRT ABI spec — GS base + W^X mprotect

**Status:** SPEC ONLY. No `rae_abi`, `syscall.rs`, or kernel file is edited by
this document. The implementation is gated on the kernel context-switch tree
cooling (the GS save/restore lands in `scheduler.rs`, which is concurrently
dirty). This is the design that lets the architect land `[interface]` + kernel
impl + RaeBridge wiring together cleanly when the tree is safe.

**Concept line served:** §Compatibility — "RaeBridge runs Windows apps natively"
and the gaming thesis ("Steam day one or there is no gaming OS"). This is the
gate from "a hand-built PE runs" (`testpe` → `ExitProcess(42)` → exit 42,
QEMU-proven) to "a real MSVC-compiled `.exe` runs."

---

## 0. Why this is the gate

A hand-assembled PE can avoid touching the TEB. A **real MSVC-CRT** binary
cannot: its `__scrt_common_main_seh` entry, before it does anything else, reads
the Thread Environment Block via `gs:[0x30]` (the TEB self-pointer) and uses
`gs`-relative TEB fields throughout — stack limits (`gs:[0x08]`/`gs:[0x10]`),
the TLS array (`gs:[0x58]`), the PEB pointer (`gs:[0x60]`), and `LastError`
(`gs:[0x68]`). `__readgsqword(0x30)` is emitted inline all over the CRT and
Win32 shims.

RaeBridge's `components/raebridge/src/ldr.rs` **already** builds an
offset-correct `Teb`/`Peb`/`RtlUserProcessParameters` set
(`ProcessEnv::build`), and `ProcessEnv::gs_base()` already returns the TEB
address. Two things are missing, both kernel/ABI work:

1. **Nothing points the CPU's user GS base at the TEB.** A guest `gs:[0x30]`
   read today hits whatever GS base the kernel left in place — not the TEB.
2. **GS base is not preserved across context switches.** Even if we wrote it
   once, the next scheduler switch-in would lose it (exactly the bug
   `Task::fs_base` was created to solve for FS/TLS).

Plus a near-term constraint: RaeShield will enforce W^X, and `SYS_MMAP` today
maps every anonymous page `PRESENT | WRITABLE | USER_ACCESSIBLE` with **no**
`NO_EXECUTE` bit — i.e. RWX. The loader copies + relocates `.text` into a
writable mapping and then needs to flip it RW→RX. There is no `mprotect` today.

---

## 1. The hard part: GS base collides with the kernel's per-CPU scheme

This is **not** a clean mirror of `fs_base`. RaeenOS uses the GS base MSRs for
its own per-CPU bookkeeping, and a naive `sys_set_gs_base` breaks SMP. The
implementer MUST understand the existing scheme before touching it.

### Current GS usage (verified in-tree)

- **`IA32_KERNEL_GS_BASE`** holds `&PERCPU_SYSCALL[cpu_id]`
  (`kernel/src/syscall.rs::init_percpu_syscall`, via `KernelGsBase::write`).
  The `syscall_handler` naked entry `swapgs`-es this into the active GS base so
  `mov gs:[0x00]`/`gs:[0x08]` reach the per-CPU syscall stack slots.
- **`IA32_GS_BASE`** (the *active*, user-visible GS base) currently holds the
  **logical CPU id as a small integer** (`gdt.rs:126`
  `GsBase::write(0)` for the BSP; `gdt.rs:207` `GsBase::write(cpu_id)` for APs).
  `gdt::current_cpu_id()` reads the active GS base and expects `0..MAX_CPUS`,
  with a defense clamp to `0` for any large value.
- Inside Rust syscall code the handler does an **extra `swapgs` pair** around
  `call syscall_handler_inner` (`syscall.rs:184`/`:187`) specifically so
  `current_cpu_id()` reads the small-integer CPU id, not the per-CPU pointer.
- `linux_syscall.rs::handle_arch_prctl(ARCH_SET_GS, …)` is **refused with
  EINVAL** today, with the comment: "GS carries the per-CPU id — honoring a
  user GS base would corrupt the per-CPU scheme." x86_64 Linux libcs use FS for
  TLS, so refusing GS is fine for the Linux ABI. **It is NOT fine for Win32**,
  which mandates GS for the TEB.

### The collision, precisely

If `sys_set_gs_base(TEB)` simply writes `IA32_GS_BASE = TEB`, then the moment a
RaeBridge guest task is running in user mode and takes an interrupt or makes a
syscall, the kernel's `current_cpu_id()` (read after the inner `swapgs`) sees
the TEB address — a large virtual address — and the defense clamp silently
returns CPU `0`. On a multi-CPU boot that mis-attributes per-CPU state. It also
means the kernel's "user GS base == CPU id" invariant is violated for that task.

### The design that resolves it

Move the CPU id **out of the active user GS base and into the per-CPU block**,
so the active GS base becomes a true per-task value (the TEB for Win32 tasks,
`0` for everyone else) and `current_cpu_id()` no longer depends on it.

Two parts:

1. **Relocate `current_cpu_id()`'s source.** Add a `cpu_id: u32` field to
   `PerCpuSyscall` (the struct already pointed at by `IA32_KERNEL_GS_BASE`).
   Inside the syscall handler, between the two `swapgs`es, the kernel GS base is
   active — so the CPU id can be read from `gs:[<cpu_id offset>]` (kernel GS) OR,
   simpler and switch-safe, `current_cpu_id()` reads it from
   `KernelGsBase::read()` (the per-CPU pointer is stable per physical CPU and
   never changes across task switches) rather than from the active GS base.
   `KernelGsBase::read()` returns `&PERCPU_SYSCALL[cpu_id]`; deref its `cpu_id`
   field. This frees the active user GS base to be a per-task value.
   - Interrupt handlers that call `current_cpu_id()` while user GS is active
     must likewise read it from the kernel-GS per-CPU block, not the active GS.
     Audit every `current_cpu_id()` caller on the interrupt path.
2. **Make the active user GS base a per-task field**, saved/restored exactly
   like `fs_base`. See §2.

This is the load-bearing decision. It is invasive (touches `gdt.rs`,
`syscall.rs` per-CPU struct, and every interrupt-path `current_cpu_id()`), which
is *why* the impl is gated on the kernel cooling — do it as one careful slice,
not interleaved with other context-switch churn.

> Alternative considered and rejected: keep CPU id in the active GS base and
> have RaeBridge guests use a software TEB shim (translate `gs:[off]` at
> instruction-decode time). Rejected — it defeats "runs Windows code natively
> in-process," forces per-instruction emulation of every CRT `gs:` access, and
> contradicts `exec.rs`'s "sys_mmap-backed memory is directly executable" model.

---

## 2. Syscall 1 — `SYS_SET_GS_BASE`

### Allocation

| field | value |
|---|---|
| number | **282** (next free after `SYS_SEARCH_QUERY_RESOLVED` = 281; SYSCALL_TABLE.md "Next free: 282") |
| name | `SYS_SET_GS_BASE` |
| rae_abi const | `pub const SYS_SET_GS_BASE: u64 = 282;` (mirror the `SYS_SET_FS_BASE = 126` entry) |
| args | `rdi = base` (user virtual address of the TEB) |
| rax | `0` on success, `u64::MAX` on a non-canonical / kernel-half address |
| cap gate | **none** — same posture as `SYS_SET_FS_BASE` (126), which is ungated. Setting your own GS base is a per-task register write into your own address space; it grants no new authority. |
| sandbox class | allowed in **every** `SandboxLevel`, including safe mode — it writes no block device, reads no secret, and a guest setting its own TEB pointer is inert to other tasks. |
| ABI_VERSION | **no bump** — additive number, no existing signature moved. |

### Semantics

Mirror `SYS_SET_FS_BASE` (`syscall.rs` arm 126) exactly:

1. Reject `base >= 0x0000_8000_0000_0000` (non-canonical / kernel half) →
   `u64::MAX`. (The TEB lives in the guest's user mapping, always below this.)
2. Persist into the new per-task field: `with_current_task_mut(|t| t.gs_base = base)`.
3. Write it live so it is in effect on syscall return. **CRITICAL — which MSR:**
   the value that is active in user mode is `IA32_GS_BASE`, but the
   `syscall_handler` does a final `swapgs` (line ~208) *before* `sysretq`, which
   swaps `IA32_GS_BASE` ↔ `IA32_KERNEL_GS_BASE`. At the point the Rust arm runs,
   the **kernel** GS base is active and the **user** GS base sits in
   `IA32_KERNEL_GS_BASE`. Therefore `SYS_SET_GS_BASE` must write the user TEB
   into **`IA32_KERNEL_GS_BASE`** (`KernelGsBase::write`), so the trailing
   `swapgs` installs it as the active GS for the returning guest. Writing
   `GsBase::write` here would be clobbered by that `swapgs`. (Symmetric to why
   `SYS_SET_FS_BASE` writes `FsBase` directly — FS has no swap.)
   - This is the single most error-prone line in the whole change. The FAIL-able
     proof (§5) exists specifically to catch a wrong-MSR write.

### Per-task field + context-switch save/restore (mirrors `fs_base`)

Add to `kernel/src/task.rs` `struct Task` (next to `pub fs_base: u64`):

```rust
/// Win32 TEB pointer for RaeBridge guests (the user-visible GS base).
/// 0 for native/Linux tasks. Saved/restored across context switches like
/// fs_base — see scheduler.rs. SYS_SET_GS_BASE writes it.
pub gs_base: u64,
```

Initialize it `0` in every `Task` constructor (the four `inherited_fs_base()`
sites at `task.rs:485/556/665/865` get a sibling `gs_base: 0`). **Do NOT** mirror
`inherited_fs_base()`'s "read the live MSR" trick — the live GS base is the
kernel per-CPU pointer mid-init, never a TEB; inheriting it would be wrong.
Default `0` = "this task has no Win32 TEB."

Then add the GS restore to **all three** context-switch sites that already do
the `fs_base` conditional write, using the **same field-vs-field, write-only-
if-changed** pattern (`scheduler.rs`):

- `yield_task` path (~line 778–831): alongside `old_fs`/`new_fs`, capture
  `old_gs = current.gs_base` / `new_gs = next.gs_base`; after the `FsBase::write`
  conditional, add `if new_gs != old_gs { KernelGsBase::write(new_gs) }`.
- `block_current_task_with` (~line 883): same.
- the exit/reap switch (~line 1240–1251, `dying_fs`): same, with `dying_gs`.

**Why `KernelGsBase::write` in the scheduler too, not `GsBase::write`:**
context switches run inside the kernel with the **kernel** GS base active (the
per-CPU pointer). The user GS base for the *incoming* task is the value that the
eventual return-to-user `swapgs` will install — i.e. it must sit in
`IA32_KERNEL_GS_BASE` while the kernel runs. So the restore writes
`KernelGsBase`. BUT — the kernel ALSO relies on `IA32_KERNEL_GS_BASE` holding
`&PERCPU_SYSCALL[cpu_id]` for the *next* `swapgs`-in on the next syscall entry.

This is the subtle interaction and it MUST be designed correctly:

> `swapgs` is its own inverse and is only ever executed at kernel/user
> boundaries (syscall entry/exit, interrupt entry/exit). While the kernel runs,
> the **active** GS base = per-CPU pointer and `IA32_KERNEL_GS_BASE` = the
> user/guest GS base. While the guest runs, the **active** GS base = the guest
> TEB and `IA32_KERNEL_GS_BASE` = per-CPU pointer.

So the per-CPU pointer is NOT a constant in `IA32_KERNEL_GS_BASE` — it is only
there *while the guest is executing*, and it is swapped back to active on the
next boundary. The context switch happens with the kernel GS active, meaning
`IA32_KERNEL_GS_BASE` currently holds the *outgoing* task's user GS base. The
restore therefore correctly overwrites it with the *incoming* task's user GS
base. On the next syscall/IRQ entry from the incoming guest, `swapgs` makes the
guest TEB active and parks the per-CPU pointer — which is exactly what
`init_percpu_syscall` placed in `IA32_KERNEL_GS_BASE` at boot and which the
boundary `swapgs`es preserve. **No conflict** — provided `current_cpu_id()` was
moved off the active GS base (§1.1). Document this lock-step in the impl.

> Implementation note on `wrmsr` cost: like `fs_base`, the write is
> conditional (`new_gs != old_gs`), so native/Linux tasks (both `0`) never pay
> the `wrmsr` on a switch — the cost lands only when scheduling to/from a
> RaeBridge guest.

> FSGSBASE note: the kernel writes GS via the MSR (`KernelGsBase::write`), not
> `wrgsbase`, matching the `arch_prctl(ARCH_SET_FS)` comment that "QEMU's
> default cpu lacks FSGSBASE." Keep the MSR path.

### `arch_prctl(ARCH_SET_GS)` follow-up (Linux ABI, separate)

Once `gs_base` is a real per-task field, `linux_syscall.rs`'s `ARCH_SET_GS`
EINVAL refusal *could* be relaxed for completeness — but Win32 guests use the
native `SYS_SET_GS_BASE`, not `arch_prctl`, so this is out of scope here. Leave
`ARCH_SET_GS` refused unless a Linux workload needs it; if relaxed later, it
writes the same `Task::gs_base` field.

---

## 3. Syscall 2 — `SYS_MPROTECT`

### Allocation

| field | value |
|---|---|
| number | **283** (next free after `SYS_SET_GS_BASE` = 282) |
| name | `SYS_MPROTECT` |
| rae_abi const | `pub const SYS_MPROTECT: u64 = 283;` |
| args | `rdi = addr`, `rsi = len`, `rdx = prot` |
| rax | `0` on success; `u64::MAX` on bad range / unmapped page / disallowed transition |
| cap gate | **none today**, but see RaeShield W^X interaction below. The operation can only narrow/adjust protections on the caller's *own* already-mapped user pages; it maps nothing new and reaches no other address space. |
| sandbox class | allowed in every `SandboxLevel` including safe mode (no block-device write; pure page-flag edit on the task's own mapping). |
| ABI_VERSION | no bump (additive number). |

### `prot` bits — reuse the existing `SYS_MMAP` convention

`SYS_MMAP` already takes `prot` in `rdx` with `3` meaning RW (see
`exec.rs::load_pe_executable` calling `sys_mmap(.., 3, ..)`). Define the bits
explicitly in `rae_abi` so both sides agree (today the kernel mmap arm ignores
`prot` and always maps RWX — `SYS_MPROTECT` is where the bits start to bite):

```rust
pub const PROT_NONE:  u64 = 0;
pub const PROT_READ:  u64 = 1;   // bit 0
pub const PROT_WRITE: u64 = 2;   // bit 1
pub const PROT_EXEC:  u64 = 4;   // bit 2
```

(`PROT_READ | PROT_WRITE == 3` matches today's mmap call site. POSIX-compatible
bit values; this is the one place a Linux numeric convention is harmless because
it is a local ABI constant, not an imported architecture.)

### Page-flag mapping

| prot | PTE flags set | notes |
|---|---|---|
| contains `PROT_WRITE` | `WRITABLE` | else cleared |
| does NOT contain `PROT_EXEC` | `NO_EXECUTE` set | else cleared (executable) |
| always (for any non-NONE) | `PRESENT \| USER_ACCESSIBLE` | RaeBridge pages are user pages |
| `PROT_NONE` | clear `PRESENT` (or `WRITABLE`+`NO_EXECUTE`, present) | spec leaves present-but-inaccessible as the simpler choice; document which |

`PROT_READ` is implicit on x86_64 (no per-page read-disable independent of
present); a present user page is always readable. So the meaningful flips are
`WRITABLE` and `NO_EXECUTE`. The W^X flip the loader needs is exactly
`PROT_READ | PROT_EXEC` (clear `WRITABLE`, clear `NO_EXECUTE`).

### Semantics / validation

1. **Page-alignment:** `addr` MUST be 4 KiB-aligned; `len` is rounded up to a
   page (reuse the `checked_add(0xFFF) & !0xFFF` overflow-safe rounding from the
   mmap arm). A non-aligned `addr` → `u64::MAX`.
2. **Range check:** `addr + len` MUST stay below `USER_SPACE_END`
   (`0x0000_8000_0000_0000`) — same guard as mmap. Reject kernel-half / overflow.
3. **Every page in the range MUST already be mapped** for the caller. Walk the
   active page table (reuse `user_leaf_flags`); a hole → `u64::MAX`, do not map
   on demand (this is `mprotect`, not `mmap`).
4. Update each PTE's `WRITABLE` / `NO_EXECUTE` per the table above, then
   **`flush` the TLB** for each page (`x86_64` `Mapper::update_flags` returns a
   `MapperFlush` — `.flush()` it). A missed TLB flush leaves the old protection
   live and is a silent W^X bypass.
5. **All-or-nothing is not required** but on a mid-range failure, document
   whether already-flipped pages are rolled back. Recommended: validate the
   WHOLE range is mapped (step 3) *before* flipping any page, so the flip loop
   cannot fail partway. This makes it atomic in practice.

### RaeShield W^X interaction

This syscall is the mechanism RaeShield's W^X policy will use, and is also the
thing the policy must *constrain*:

- **Today (W^X not yet enforced):** `SYS_MMAP` maps RWX, so the RW→RX flip is an
  optional hardening step; the guest would run even without it.
- **When RaeShield enforces W^X (Phase 9):** `SYS_MMAP` should stop setting the
  execute permission by default (map RW + `NO_EXECUTE`), and `SYS_MPROTECT`
  becomes the *only* way to make a page executable — and the policy gate lives
  HERE: a `PROT_WRITE | PROT_EXEC` request (W+X simultaneously) is the thing
  RaeShield refuses. The spec'd rule:

  > `SYS_MPROTECT` with both `PROT_WRITE` and `PROT_EXEC` set is **rejected**
  > under an active W^X policy (`u64::MAX`). A page may be writable OR
  > executable, never both at once. RW→RX (drop write, add exec) and RX→RW (the
  > JIT/relocation re-patch case) are the allowed transitions.

  RaeBridge's loader fits this: it mmaps RW, copies+relocates, then mprotects
  `.text` to `PROT_READ | PROT_EXEC` (W cleared as X is set) — never W+X.
- Until the policy lands, `SYS_MPROTECT` honors the requested bits verbatim
  (including W+X) so bring-up isn't blocked; the W^X refusal is a one-line gate
  added when RaeShield flips enforcement on. Flag this clearly so the gate is
  not forgotten (`// MasterChecklist Phase 9: refuse W+X under W^X policy`).

---

## 4. The RaeBridge-side sequence

This is the order `exec.rs::load_pe_executable` (and the not-yet-written
"spawn + jump to entry" step its docstring names) wires up. Steps 1–4 already
exist; 4b/5/6 are the new wiring on top of the two new syscalls.

```
1. parse PE                       (pe_loader::parse_pe — done)
2. mmap RW at preferred base       sys_mmap(base, size, PROT_READ|PROT_WRITE, ..)
                                   (today prot=3; unchanged)
3. copy headers + sections; relocate if rebased; patch IAT
                                   (done — image is writable here)
4. parse .pdata for SEH            (done — seh::parse_pdata)
--- new wiring ---
4b. for each executable section (Characteristics & IMAGE_SCN_MEM_EXECUTE,
    e.g. .text): page-align [sec_va, sec_va+virtual_size) and
        sys_mprotect(sec_va, sec_size, PROT_READ | PROT_EXEC)
    Writable data sections (.data/.bss) stay RW; read-only data (.rdata)
    may be flipped to PROT_READ for hardening. Do the .text flip AFTER all
    copies+relocs+IAT patches (step 3) — flipping before relocation would
    fault the relocation writes.
5. build TEB/PEB                   ProcessEnv::build(image_base, stack_base,
                                   stack_limit, cmdline) — done; returns a
                                   Box whose TEB self_ptr == gs_base().
6. set_gs_base(TEB)                sys_set_gs_base(env.gs_base())
                                   AFTER this, the running thread's gs:[0x30]
                                   reads env.teb.self_ptr (== the TEB addr),
                                   gs:[0x60] reads the PEB, gs:[0x68] LastError.
7. jump to entry                   transfer control to ExecutablePe.entry_point
                                   with the Win64 ABI initial state (rsp =
                                   16-byte-aligned guest stack, shadow space).
```

After step 6, a guest `__readgsqword(0x30)` (or raw `mov rax, gs:[0x30]`)
returns the TEB self-pointer, and every subsequent `gs:[off]` TEB access in the
MSVC CRT and the Win32 shims resolves against the `ldr.rs`-built structures. The
self-pointer consistency `env.teb.self_ptr == env.gs_base()` is already asserted
in `ldr.rs` tests (`assert_eq!(env.teb.self_ptr, env.gs_base())`).

**Lifetime constraint (carry into the wiring):** the `Box<ProcessEnv>` whose
address is handed to `sys_set_gs_base` MUST outlive the guest thread. RaeBridge
runs the guest in-process, so the `ProcessEnv` must be owned by the long-lived
per-process bridge state, not a stack temporary — dropping it dangles the GS
base. (This is a RaeBridge-side correctness note, not a kernel one.)

---

## 5. FAIL-able proof

The proof that survives review is a **tiny MSVC-style PE** (built with the
existing `testpe.rs` PE synthesizer, not the C toolchain) whose entry point:

1. `mov rax, gs:[0x30]` — read the TEB self-pointer.
2. compare it to the known TEB address the loader set (passed via a register or
   a fixed data slot the harness seeds), and
3. **deliberately yield / make a blocking syscall** (e.g. `SYS_YIELD` 28, or a
   short `SYS_RECV`) to force a context switch, THEN
4. re-read `gs:[0x30]` and confirm it still equals the TEB (proves save/restore),
5. `ExitProcess(code)` where `code` encodes pass (TEB matched both before AND
   after the switch) vs fail (mismatch → a distinct non-zero code).

The harness asserts the guest exits with the pass code. A wrong-MSR write in
`SYS_SET_GS_BASE` (writing `GsBase` instead of `KernelGsBase`) fails step 1
(TEB never visible). A missing scheduler restore fails step 4 (TEB lost after
the switch). Both failure modes print a distinct exit code — the test can FAIL,
which is the bar (CLAUDE.md rule 16).

Layering (cheapest first, per TESTING_STRATEGY):
- **Host KAT** for the W^X flag math: a pure function `prot_to_pte_flags(prot)
  -> (writable, nx)` unit-tested in `kernel` or a shared helper, asserting
  `PROT_READ|PROT_EXEC -> (false, false)` (RX), `PROT_READ|PROT_WRITE ->
  (true, true)` (RW+NX), and W+X rejection under policy. Catches the bit logic
  off-target.
- **Boot smoketest** (R10): `raebridge`'s boot smoketest loads the tiny gs-PE,
  runs it through a single self-yield, and prints
  `[raebridge] gs-teb smoketest: read_before=.. survived_switch=.. -> PASS/FAIL`.
- **QEMU CI**: the above marker present, no PANIC, `System successfully booted`.
- **iron**: same marker on Athena once the kernel slice lands.

---

## 6. SEH live-delivery — NAMED, not designed here

A real CRT binary that faults (AV / divide / `__try`/`__except`) needs the
kernel to deliver the fault to the guest's `__C_specific_handler` via the
`.pdata`/`.xdata` unwind tables RaeBridge already parses
(`seh::parse_pdata`/`seh.rs`, host-KAT'd 14/14 per the SEH-engine memory). That
requires kernel signal/fault plumbing: a CPU fault in a RaeBridge guest must be
trapped, translated to an `EXCEPTION_RECORD` + `CONTEXT`, and the guest's
language handler invoked on the guest stack, with continue/unwind semantics.

**That is a SEPARATE future spec** (`docs/research/raebridge-seh-delivery-abi.md`,
to be written). It is the next gate AFTER this one — a CRT binary first has to
*start* (TEB + W^X, this doc) before its fault path matters. Not designed here.

---

## 7. Hand-off

| piece | owner | deliverable |
|---|---|---|
| `[interface]` commit | **architect (opus, sole `rae_abi` editor)** | `SYS_SET_GS_BASE = 282`, `SYS_MPROTECT = 283`, `PROT_*` consts in `rae_abi`; rows in `docs/SYSCALL_TABLE.md` (new Block 33); ungated, all-sandbox, no `ABI_VERSION` bump (additive numbers). Lands WITH the dispatch arms. |
| kernel impl | **raeen-kernel** | (a) `Task::gs_base` field + `0` init at the 4 ctor sites; (b) the dispatch arms 282/283 in `syscall.rs` (282 writes `KernelGsBase`; 283 walks+flips PTEs + TLB flush); (c) GS save/restore at the 3 `scheduler.rs` switch sites mirroring `fs_base`; (d) move `current_cpu_id()` off the active GS base onto the kernel-GS per-CPU `cpu_id` field, and audit interrupt-path callers. **Gated on the context-switch tree cooling** (touches `scheduler.rs`). |
| RaeBridge wiring | **raeen-compat** | `syscalls.rs` wrappers `sys_set_gs_base`/`sys_mprotect`; `exec.rs` steps 4b/5/6 (mprotect `.text` RX, `set_gs_base(env.gs_base())`, jump entry); own the `ProcessEnv` lifetime; the tiny gs-PE FAIL-able smoketest + host KAT for the prot math. |

The architect lands the `[interface]` (constants + table) + the kernel dispatch
+ the scheduler restore as one coordinated change once the kernel context-switch
files are clean, then raeen-compat wires the loader against the published
numbers (282 / 283). This unblocks real MSVC-compiled Windows `.exe` execution.
