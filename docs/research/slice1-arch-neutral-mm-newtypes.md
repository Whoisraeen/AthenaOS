# Spec: Slice 1 — arch-neutral memory-address newtypes (`arch::PhysAddr` / `VirtAddr` / `Frame`)

Status: RESEARCH / executable plan. **SPEC ONLY — this document touches no kernel code, no
`Cargo.toml`, no `xtask`, no `ath_abi`.** It is the concrete, sub-sliced implementation plan for
the "Slice 1" the multi-arch spec defined as *"arch-neutral PhysAddr/VirtAddr + the paging trait"*
(`docs/research/aarch64-bringup-spec.md` §"Slice 1"). It **extends, does not re-derive**,
`docs/research/multi-arch-abstraction.md` and ADR 0007 — and it follows on from the four landed,
verifier-confirmed `arch::` seam relocations (0b-1 IDT-install, 0b-2 BSP-GDT, 0b-3 EOI,
0b-4 timer-arm).

Owner of the seam contract: **athena-architect** (this is an internal kernel HAL type contract,
NOT `ath_abi` — `ABI_VERSION` unchanged, §6 below). Implementer: **athena-kernel**.

---

## Concept promise served

Same north-star clause the four landed seams quote, already verbatim in `kernel/src/arch/mod.rs`:

> "AthenaOS refuses ISA lock-in: the kernel sits on a clean `arch::` abstraction layer … so the
> same OS boots x86_64, aarch64 (ARM 64-bit), and i686 (32-bit x86) — each proven independently."
> (LEGACY_GAMING_CONCEPT.md §Architecture Reach)

Slice 1 is the load-bearing precursor of that clause's MM half. The four seams relocated *calls*
(install IDT, load GDT, EOI, arm timer). Slice 1 relocates a *type*: the `x86_64` crate's
`PhysAddr`/`VirtAddr` are x86-specific value types that **do not exist on aarch64** and that
currently leak into ~21 shared-kernel files. Until they are arch-neutral, no aarch64 backend can
provide its own 48-bit-VA / TTBR-based address representation without forking the shared MM logic.
ADR 0007 §4 makes this an explicit gate: *"No aarch64 code until Slice 1 lands."*

---

## 1. The fan-out reality (grep-quantified — this is the size of the migration)

The migration target splits into **two distinct clusters**. Slice 1 owns ONLY the first; the
second is the paging-trait work this spec recommends deferring (§4).

### Cluster A — address VALUE types `PhysAddr` / `VirtAddr` (Slice 1 scope)

`\b(PhysAddr|VirtAddr)\b` → **166 occurrences across 21 files**:

| File | hits | role |
|---|---|---|
| `kernel/src/memory.rs` | 41 | MM core — public signatures (`kernel_translate_addr`, `alloc_kernel_stack`, `phys_to_virt`, `virt_to_phys`, `map_mmio_region`, `pin_memory`, …) |
| `kernel/src/syscall.rs` | 21 | dispatch helpers (mmap/mprotect/brk addr math) |
| `kernel/src/task.rs` | 16 | task entry/stack addresses |
| `kernel/src/scheduler.rs` | 11 | per-task stack/entry pointers |
| `kernel/src/gdt.rs` | 9 | TSS/IST stack addresses (already partly behind `arch::cpu`) |
| `kernel/src/smp.rs` | 8 | AP trampoline / per-CPU stack addresses |
| `kernel/src/posix.rs` | 7 | brk/mmap addr math |
| `kernel/src/compositor.rs` | 6 | framebuffer/surface phys+virt |
| `kernel/src/elf.rs` | 4 | segment load addresses |
| `kernel/src/xhci.rs` | 4 | DMA ring virt/phys |
| `kernel/src/iommu.rs` | 4 | IOVA/phys |
| `kernel/src/game_session.rs`, `hardening.rs` | 3 each | — |
| `linux_compat.rs`, `memory/buddy.rs`, `memory/allocator.rs` | 2–3 each | — |
| `ipc.rs`, `linux_syscall.rs`, `main.rs`, `virtio.rs` | 1 each | — |

Of these, the explicit **`use x86_64::{…VirtAddr/PhysAddr}` import lines number only 26** (listed
by grep). The other ~140 hits are **method calls on the values** — `VirtAddr::new(...)`,
`.as_u64()`, `.as_mut_ptr::<T>()`, `.align_up()`, `.align_down()`, `PhysAddr::new(...)`. That API
surface (`(VirtAddr|PhysAddr)::new | .as_u64() | new_truncate | .as_mut_ptr() | .align_up/down`)
appears **396 times across 58 files** — but the vast majority of those 58 files use the value
type *transitively* via `memory.rs`'s return types, not by naming `x86_64::` themselves. **The 21
files above are the true edit surface for Slice 1.**

### Cluster B — paging-TABLE types (NOT Slice 1 — the paging-trait, §4)

`\b(PhysFrame|OffsetPageTable|PageTable|PageTableFlags|Mapper|FrameAllocator)\b` →
**319 occurrences across 26 files**, heavily concentrated:

| File | hits |
|---|---|
| `kernel/src/memory.rs` | 134 |
| `kernel/src/posix.rs` | 38 |
| `kernel/src/syscall.rs` | 35 |
| `kernel/src/tpm.rs` | 24 |
| `kernel/src/smp.rs` | 17 |
| `kernel/src/numa.rs` | 11 |
| everything else | ≤5 each |

This cluster is the page-table *manipulation* surface (map/unmap/translate, `Cr3`,
`OffsetPageTable`). It is the §10 paging keystone, the highest-risk migration, and it is where the
concurrently-dirty `sys_mprotect` work lives (`syscall.rs`/`posix.rs`). **§4 recommends it as its
own later slice.** `Frame` appears in Slice 1 only as a thin alias (a `PhysFrame`-shaped handle)
so the *type name* is arch-neutral; the page-table *operations* that consume it do NOT move in
Slice 1.

### Honest verdict on size

Cluster A is **medium fan-out, low risk** — 21 files, mostly value-type method calls that are
behavior-identical under a transparent alias. It is **too large for one atomic diff** (a 21-file
single commit cannot be reviewed as "obviously zero-behavior"), but it sub-slices cleanly along
file/subsystem lines (§3). This is **several small sub-slices, not one** — matching the
seam-relocation discipline (each individually verifiable, x86 stays 7/7).

---

## 2. The newtype design — RECOMMENDATION: **(a) transparent type-aliases now, newtype-ready later**

Three options were weighed against the house tie-breaker (Concept reach > beats-the-alternatives >
simplest-correct-now > reversibility):

### Option (a) — transparent type-aliases to the `x86_64` crate types *for x86_64*

```rust
// kernel/src/arch/x86_64/addr.rs  (NEW)
pub type PhysAddr = x86_64::PhysAddr;
pub type VirtAddr = x86_64::VirtAddr;
pub type Frame    = x86_64::structures::paging::PhysFrame<x86_64::structures::paging::Size4KiB>;
```
re-exported as `arch::PhysAddr` / `arch::VirtAddr` / `arch::Frame`. Shared code writes
`arch::VirtAddr`; on x86_64 it *is* `x86_64::VirtAddr`, so every existing method
(`.as_u64()`, `::new()`, `.as_mut_ptr()`, `.align_up()`) works **byte-identically, zero behavior
change, zero codegen change**. The aarch64 backend later defines its OWN `arch::VirtAddr` (a real
48-bit-VA newtype or a wrapper over a future `aarch64`-crate type) behind the same name.

- **Reach:** full — gives aarch64 a slot, kills the `x86_64::VirtAddr` name from shared code.
- **Simplest-correct-now:** strongest — the migration is a pure *import rewrite*
  (`use x86_64::VirtAddr` → `use crate::arch::VirtAddr`), no method-call edits, no semantic risk.
- **Reversibility:** trivial — an alias is deletable; revert = restore the import.
- **Weakness:** an alias does NOT *enforce* arch-neutrality. Shared code could still call an
  x86-only inherent method (e.g. `VirtAddr::new_truncate`) that aarch64's type won't have. This is
  caught later — when aarch64's backend defines a non-identical `VirtAddr`, the x86-only method
  call fails to compile on the aarch64 build (CI's aarch64 *build* job, spec Slice A2). So the
  alias makes the END state's divergence a **compile error on aarch64**, never a silent runtime
  bug. Acceptable: the alias is the *path*, not the *end state*.

### Option (b) — real newtype wrappers now (`pub struct VirtAddr(u64)`)

True arch-neutrality immediately: a shared `arch::VirtAddr(u64)` with re-implemented
`new/as_u64/as_mut_ptr/align_up/align_down`, each x86 backend providing canonical-form checks.

- **Reach/control:** highest — shared code physically cannot touch an x86-ism.
- **Cost:** every one of the ~140 method-call sites must be audited against the new inherent API,
  and `memory.rs` must convert at the `x86_64`-crate boundary (the page-table calls still want the
  real `x86_64::VirtAddr`). That conversion layer is exactly the risk the seam-relocation
  discipline avoids — it is NOT "obviously zero-behavior."
- **Verdict:** correct *eventually*, but doing it now front-loads the highest-risk edit before any
  aarch64 code exists to justify the exact API shape. Premature.

### Option (c) — trait-based associated types (`trait Arch { type VirtAddr; … }`)

A `dyn`-free associated-type per arch.

- **Verdict:** rejected for Slice 1. ADR 0007 already chose monomorphized free-fns/assoc-types for
  the *seam*, but an associated *value type* threaded through 21 files' signatures adds generic
  noise (`<A: Arch>` bounds or a fixed `type Active`) for zero benefit over the alias while x86 is
  the only backend. The alias gives the same compile-time monomorphization with none of the
  signature churn. Revisit only if a future need to be generic over *two simultaneously-compiled*
  arches appears (it will not — `#[cfg(target_arch)]` selects exactly one).

### RECOMMENDATION

**Option (a): transparent type-aliases, migrated incrementally, newtype-promotable later.**

Rationale by tie-breaker: it achieves the **same Concept reach** as (b) (aarch64 gets its slot,
shared code stops naming `x86_64::`), it is the **simplest correct path now** (import rewrite, not
semantic rewrite), and it is the **most reversible**. The one thing it defers — *enforcing*
neutrality — is enforced for free the moment aarch64's backend lands a non-identical type, by the
aarch64 build job. The END-state guarantee the brief requires ("shared code must NOT expose
`x86_64::PhysAddr`") is met by the alias: after migration, **no shared file names `x86_64::` for
these types** — they name `arch::`. The promotion from alias to newtype (if aarch64 ever wants
shared code to use *only* a common inherent API) is itself a later, independently-verifiable slice
that touches only `arch/x86_64/addr.rs` + any x86-only method call it flushes out.

**Names:** `arch::PhysAddr`, `arch::VirtAddr`, `arch::Frame`. (`Frame` not `PhysFrame` — drop the
x86 "Phys" framing noun; a `Frame` is a page-aligned physical allocation unit on every arch.
`Page` is deliberately NOT introduced in Slice 1 — it is a page-table-walk concept that belongs to
the paging-trait slice, §4.)

---

## 3. Incremental migration strategy (mirrors the seam-slice discipline)

Each sub-slice is a **separately-committable, separately-verifiable, zero-behavior-change** step
that keeps x86 booting 7/7. Proof line for EVERY sub-slice below is the same x86 baseline:

> `[ OS ] System successfully booted.` + `boot health: 6/6 critical PASS -> HEALTHY`, **no
> `[PANIC]`**, `[BOOT-BENCH]` not regressed, and the **arch smoketest line** (see §5) still prints
> `… -> PASS`. SMP-touching sub-slices (1f smp.rs, 1g task/scheduler) get **≥5 boots at
> `ATHENA_SMP=1` and `=2`** (CLAUDE.md §17).

Sub-slices are ordered **lightest-and-most-isolated first**, with the concurrently-dirty
`memory.rs` / `syscall.rs` / `posix.rs` files sequenced **last** so they wait for a clear lane
(the concurrent's `sys_mprotect`-area work finishes first; coordinate via the lead).

### Sub-slice 1a — define the `arch::` address types + a FAIL-able smoketest (x86 unchanged)
- **Files:** `kernel/src/arch/x86_64/addr.rs` (NEW — the three aliases), `arch/x86_64/mod.rs`
  (`pub mod addr; pub use addr::{PhysAddr, VirtAddr, Frame};`), `arch/mod.rs` (extend the
  smoketest + `dump_text` to report the address-type seam). **No shared caller touched yet.**
- **arch:: surface added:** `arch::PhysAddr`, `arch::VirtAddr`, `arch::Frame` + a
  `arch::addr::roundtrip_ok()` predicate the smoketest asserts.
- **Why first:** it is the seam-definition step — pure addition, nothing migrated, immediately
  "still 7/7." It mirrors how 0b-1 first defined `interrupts::vectors_installed()` before anything
  depended on it. **This is the named hand-off (§7).**
- **Proof line:** baseline + a new smoketest token (§5): `… addr=roundtrip-ok …`.

### Sub-slice 1b — migrate the leaf driver/IO callers (lowest fan-out, no MM coupling)
- **Files (1–4 hits each, leaf consumers):** `xhci.rs`, `iommu.rs`, `virtio.rs`,
  `linux_compat.rs`, `linux_syscall.rs`, `compositor.rs`. Rewrite `use x86_64::{…VirtAddr/PhysAddr}`
  → `use crate::arch::{VirtAddr, PhysAddr}`. Zero method-call edits (alias = same methods).
- **Why second:** these are isolated, non-SMP, non-MM-core — the cheapest real migration, proving
  the alias rewrite is mechanical before touching hot files.
- **Proof line:** baseline. (Can be split further if any single file is contentious.)

### Sub-slice 1c — `gdt.rs` (already partly behind `arch::cpu`)
- **Files:** `gdt.rs` (9 hits — TSS/IST stack addresses). It already lives behind the `arch::cpu`
  seam (0b-2), so finishing its address types is a natural, contained step.
- **Proof line:** baseline. (No SMP path here — only the BSP GDT was relocated in 0b-2; AP paths
  stay put, consistent with 0b-2's scope.)

### Sub-slice 1d — `elf.rs` + `game_session.rs` + `hardening.rs` (load-address consumers)
- **Files:** `elf.rs` (4), `game_session.rs` (3), `hardening.rs` (3). Segment/load addresses,
  no page-table ops.
- **Proof line:** baseline.

### Sub-slice 1e — `memory/buddy.rs` + `memory/allocator.rs` (frame/heap edges of MM)
- **Files:** `memory/buddy.rs` (PhysAddr in the buddy free-list), `memory/allocator.rs` (VirtAddr
  in the heap). These are the MM *sub-modules*, NOT `memory.rs` itself — they touch the address
  value type but not the `OffsetPageTable` paging core.
- **Proof line:** baseline.

### Sub-slice 1f — `smp.rs` (SMP path — ≥5 boots)
- **Files:** `smp.rs` (8 address hits). AP trampoline + per-CPU stacks.
- **Keystone watch:** the AP per-CPU syscall/exception-stack rule (§10.6) — this sub-slice only
  rewrites the *address type name*, it must NOT alter any `set_syscall_kernel_stack`/`rsp0` logic.
  Pure import rewrite only.
- **Proof line:** baseline **+ ≥5 boots at `ATHENA_SMP=1` and `=2`** (SMP path).

### Sub-slice 1g — `task.rs` + `scheduler.rs` (task entry/stack addresses — ≥5 boots)
- **Files:** `task.rs` (16), `scheduler.rs` (11). Task entry points + per-task stack addresses.
- **Keystone watch:** the yield-path lock-drop + block-path syscall-stack rule (§10.6) — again,
  type-name rewrite ONLY; no scheduler control-flow change.
- **Proof line:** baseline **+ ≥5 boots `ATHENA_SMP=1`/`=2`**.

### Sub-slice 1h — `memory.rs` public signatures (LATE — waits for the clear MM lane)
- **Files:** `memory.rs` (41 address hits). Convert the public signatures
  (`kernel_translate_addr`, `phys_to_virt`, `virt_to_phys`, `map_mmio_region`, `alloc_kernel_stack`,
  `pin_memory`/`unpin_memory`, `allocate_contiguous_frames`/`deallocate_contiguous_frames`) to
  `arch::{PhysAddr, VirtAddr, Frame}`. Because every caller above already imports `arch::` (1b–1g),
  this is the keystone that flips the MM *surface* without touching paging operations.
- **CONCURRENCY GATE:** `memory.rs` may be concurrently dirty (the brief flags `sys_mprotect`-area
  work). **Sequence this sub-slice AFTER that work merges** — coordinate with the lead; do NOT
  rebase on top of an in-flight `memory.rs`. If the lane is not clear, 1h waits; 1a–1g are
  independent of it and proceed.
- **Scope discipline:** convert ONLY the `PhysAddr`/`VirtAddr`/(Frame-as-handle) value types. Do
  NOT touch `OffsetPageTable`/`PageTableFlags`/`Mapper`/`Cr3` here — that is §4's paging-trait
  slice. The 134 Cluster-B hits in `memory.rs` stay `x86_64::` until then.
- **Proof line:** baseline **+ ≥5 boots `ATHENA_SMP=1`/`=2`** (MM core touches every path).

### Sub-slice 1i — `syscall.rs` + `posix.rs` (LATEST — the concurrent's lane)
- **Files:** `syscall.rs` (21 address hits), `posix.rs` (7 address hits). mmap/mprotect/brk addr
  math.
- **CONCURRENCY GATE:** these are the files the concurrent `sys_mprotect` work edits directly.
  **This is the LAST sub-slice and lands only after that work is merged and stable.** It is the
  cleanup tail, not a blocker for 1a–1h.
- **Scope discipline:** address value types only — the page-table flag/`Mapper` Cluster-B hits in
  these files stay for §4.
- **Proof line:** baseline **+ ≥5 boots `ATHENA_SMP=1`/`=2`**.

### Completion check for Slice 1
After 1a–1i: **no shared kernel file names `x86_64::PhysAddr` or `x86_64::VirtAddr`** (grep returns
only `arch/x86_64/addr.rs`). That grep is the falsifiable "Slice 1 done" gate. Cluster-B
(paging-table types) intentionally remains — it is §4's slice.

---

## 4. The paging-trait question — RECOMMENDATION: **DEFER to its own later slice**

The aarch64 spec's "Slice 1" phrase bundles *"arch-neutral PhysAddr/VirtAddr + the paging trait."*
This spec **splits them**: Slice 1 = address TYPES only; the **paging trait
(`map`/`unmap`/`translate`/`AddressSpace`/`switch_to`/`kernel_root`) is a SEPARATE, LATER slice.**

Rationale:
- **It is the §10 keystone and the highest risk in the tree.** Page-table manipulation is where
  the worst historical bugs live (the multi-page contiguous-frame rule §10.7, the `kernel_translate_addr`/
  `KERNEL_PML4`-not-user-CR3 rule §10.2, the ELF-spawn frame-collision class). Folding it into the
  low-risk address-type rename would make Slice 1 un-reviewable as "obviously zero-behavior" and
  blow the seam-slice discipline.
- **It is Cluster B** — 319 occurrences, 134 in `memory.rs` alone, deeply entangled with the
  `x86_64` crate's `OffsetPageTable`/`Mapper` trait machinery (which has no aarch64 analogue and
  must be replaced by a hand-written `arch::mmu::AddressSpace`, not aliased). That is genuine new
  mechanism, not a rename.
- **It collides hardest with the concurrent `sys_mprotect` work** (which IS page-table flag
  manipulation). Deferring keeps Slice 1 out of that lane almost entirely (only the address-type
  tail 1i touches those files).
- **ADR 0007's gate is satisfied by the address types alone.** "No aarch64 code until Slice 1
  lands" is about giving aarch64 a *type slot* so the boundary doesn't ossify around x86 address
  representation. The paging *trait* is itself aarch64 work (spec Slice A4 builds the aarch64 MMU)
  — it is the FIRST thing the aarch64 backend implements, so its arch-neutral shape is best
  designed *with* the aarch64 MMU in hand, not speculatively now.

**Recommendation to the lead:** rename the aarch64-spec "Slice 1" to **"Slice 1 (address types)"**
and add **"Slice 1.5 (paging trait — `arch::mmu::AddressSpace` + map/unmap/translate)"** as its own
spec/round, sequenced after Slice 1 completes and after the `sys_mprotect` work merges. Slice 1.5
gets its own design doc (it is large enough to warrant one, like the aarch64 boot spec did).

---

## 5. R10 + proof — the FAIL-able smoketest

Sub-slice 1a extends the existing `arch::run_boot_smoketest()` (the same function the four landed
seams report through) with an **address-type round-trip assertion**, and `arch::dump_text()`
(`/proc/athena/arch`) with the seam's status. The assertion (pure arithmetic, also host-KAT-able
per CLAUDE.md §15 *before* boot):

- **Identity-map round-trip:** take a known kernel phys address `p`, compute its higher-half virt
  via the kernel's physical-memory-offset (`phys_to_virt`), then map back (`virt_to_phys` /
  `kernel_translate_addr`); assert the recovered phys equals `p`. FAIL-able: a wrong alias, a
  broken offset constant, or a non-canonical truncation prints `-> FAIL`.
- **Align math:** assert `VirtAddr::new(x).align_down(PAGE_SIZE) <= x` and
  `align_up(PAGE_SIZE) >= x` and both are page-aligned, for a non-aligned `x`. FAIL-able: a wrong
  alias whose align semantics differ prints `-> FAIL`.
- **Frame alignment:** assert an `arch::Frame::start_address()` is `PAGE_SIZE`-aligned. FAIL-able.

The smoketest line gains a token (extending the existing
`name=… ptr=… if-save/restore=… vectors=… gdt=… eoi=… timer=… -> PASS` line):

```
[arch] smoketest: name=x86_64 ptr=64 … timer=exercised(pre-calib) addr=roundtrip-ok -> PASS
```

If any address assertion fails: `… addr=ROUNDTRIP-BAD -> FAIL`. This is the falsifiable proof — a
test that *can* print FAIL (CLAUDE.md §16). The host-KAT (dev-box `cargo test`) runs the same
arithmetic behind the alias first, the cheapest real proof.

**aarch64 counterpart note (per type, for the future backend):**
- `arch::VirtAddr` on aarch64 = a **48-bit VA** newtype: bits [63:48] must be all-ones (TTBR1,
  kernel/higher half) or all-zeros (TTBR0, user/lower half) — the canonical-form check differs
  from x86's bit-47-sign-extend-to-57/48. The alias hides this on x86; the aarch64 backend's
  `VirtAddr::new` enforces the aarch64 canonical form.
- `arch::PhysAddr` on aarch64 = up to 48-bit PA (IPA size from `ID_AA64MMFR0_EL1.PARange`); the
  round-trip uses the aarch64 physical-memory-offset.
- `arch::Frame` on aarch64 = a 4 KiB-granule physical frame (same `PAGE_SIZE=4096`); the
  `start_address()` alignment assertion ports unchanged.
- The **TTBR0/1 split** is a *paging-trait* concern (§4), NOT an address-type concern — the address
  newtypes only need to know "which half am I canonical for," which is encoded in the high bits, so
  Slice 1's types are aarch64-ready without naming TTBR.

The `arch/x86_64/addr.rs` docstring MUST quote the §Architecture-Reach clause (R10 artifact 4), as
the four landed seam modules do.

---

## 6. Interface note — internal HAL types, NOT `ath_abi`

These are **internal kernel HAL types**. Confirm and record:
- **`ABI_VERSION` is UNCHANGED.** Per ADR 0009 (and ADR 0007 §2: "the user/syscall ABI stays
  arch-neutral — NO `ath_abi`/`ABI_VERSION` change"), the `arch::` address newtypes never cross the
  syscall boundary. No `ath_abi` / `ath_driver_api` edit. The architecture-gate's `[interface]`
  sign-off requirement does NOT apply (it gates `ath_abi`/`ath_driver_api`, not internal `arch::`).
- **Syscall ABI surfaces raw integers, never the newtype.** Any syscall that takes/returns an
  address (mmap, mprotect, brk, `sys_claim_device` packing) MUST keep its ABI-level type a **plain
  `u64`/`usize` integer**. The newtype is constructed *inside* the kernel handler from the raw
  integer arg and never appears in a `ath_abi` constant or struct. The migration sub-slices 1h/1i
  must preserve this: the `arch::VirtAddr` lives in the handler body, the syscall *signature* stays
  integer. (This is already true today — `x86_64::VirtAddr` is constructed inside handlers, not in
  `ath_abi`; the alias preserves it.)
- The widened `arch::` surface (three type names + the `addr::roundtrip_ok()` predicate) is the
  internal seam contract athena-architect owns as documentation — consistent with how the four
  landed seams' `pub fn` surfaces are owned.

---

## 7. HAND-OFF — the named first sub-slice for athena-kernel

**Execute FIRST: Sub-slice 1a — define the `arch::` address types + extend the smoketest, x86
unchanged.**

- **Why this one:** it is the smallest, immediately-verifiable step — pure *addition* (define three
  aliases + one smoketest assertion), **no shared caller migrated yet**, so the proof is "x86 STILL
  7/7 and the new `addr=roundtrip-ok` token prints PASS." It mirrors exactly how each of the four
  landed seams opened (define the predicate + smoketest hook before any dependent edit). It carries
  zero MM-core risk and does not touch the concurrently-dirty `memory.rs`/`syscall.rs`/`posix.rs`.
- **Files:**
  - `kernel/src/arch/x86_64/addr.rs` — **NEW**: `pub type PhysAddr = x86_64::PhysAddr;`
    `pub type VirtAddr = x86_64::VirtAddr;`
    `pub type Frame = x86_64::structures::paging::PhysFrame<x86_64::structures::paging::Size4KiB>;`
    + a `pub fn roundtrip_ok() -> bool` (the host-KAT-able align + identity-map-offset arithmetic)
    + the §Architecture-Reach R10 docstring.
  - `kernel/src/arch/x86_64/mod.rs` — add `pub mod addr;` and
    `pub use addr::{PhysAddr, VirtAddr, Frame};`.
  - `kernel/src/arch/mod.rs` — extend `run_boot_smoketest()` to call `addr::roundtrip_ok()` and
    append the `addr=roundtrip-ok|ROUNDTRIP-BAD` token to the smoketest line; extend `dump_text()`
    with an `address_types: arch::{PhysAddr,VirtAddr,Frame}` line and move `paging` to the
    `seams_pending` note (it already is).
- **Exact boot-log proof line** (the falsifiable gate):
  ```
  [arch] smoketest: name=x86_64 ptr=64 if-save/restore=ok vectors=installed gdt=loaded eoi=exercised timer=exercised(pre-calib) addr=roundtrip-ok -> PASS
  ```
  plus the unchanged `[ OS ] System successfully booted.` + `boot health: 6/6 critical PASS ->
  HEALTHY`, no `[PANIC]`, `[BOOT-BENCH]` not regressed. Host-KAT the `roundtrip_ok()` arithmetic on
  the dev box first (`cargo test`), then QEMU boot.
- **Then 1b → 1i** per §3, with **1h (`memory.rs`) and 1i (`syscall.rs`/`posix.rs`) sequenced after
  the concurrent `sys_mprotect` work merges** (coordinate via the lead). **No aarch64 code until the
  full Slice 1 (through 1i) lands** (ADR 0007 gate). **The paging trait is Slice 1.5, deferred** (§4).

---
Sources:
- `docs/research/aarch64-bringup-spec.md` §"Slice 1" + §"The seam list"
- `docs/research/multi-arch-abstraction.md` §"The seam list" / §3
- `docs/decisions/0007-multi-arch-strategy.md` §4 (the "no aarch64 until Slice 1" gate)
- `kernel/src/arch/mod.rs` + `arch/x86_64/mod.rs` (the four landed seam relocations 0b-1..0b-4)
- grep fan-out: `kernel/src` — PhysAddr/VirtAddr = 166 hits / 21 files; paging-table types = 319
  hits / 26 files (counts in §1)
