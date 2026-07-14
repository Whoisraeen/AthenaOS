# Spec: Slice 1.5 — arch-neutral PAGING trait (`arch::mmu::AddressSpace`)

Status: RESEARCH / executable plan. **SPEC ONLY — this document touches no kernel code, no
`Cargo.toml`, no `xtask`, no `rae_abi`.** It is the §10-keystone follow-on the Slice-1 spec
explicitly deferred (`docs/research/slice1-arch-neutral-mm-newtypes.md` §4: *"the paging trait is
Slice 1.5, deferred"*) and the **last big arch-abstraction piece before the aarch64 backend can
compile + boot** (it is aarch64 Slice A4's first task — the MMU bring-up; this trait is the seam
A4 plugs into).

Owner of the seam contract: **raeen-architect** (internal kernel HAL — NOT `rae_abi`,
`ABI_VERSION` unchanged, §6). Implementer: **raeen-kernel**.

**Precondition:** Slice 1 (`arch::PhysAddr`/`VirtAddr`/`Frame` aliases) is COMPLETE + verifier-
confirmed (commit `f809f41`). This spec builds on those types; it does NOT re-derive them. The
aarch64 descriptor encoder + `TCR_EL1`/`MAIR_EL1` builders already exist, host-KAT'd, in
`components/aarch64_logic/src/mmu.rs` (commit `a4c9f5b`) — the aarch64 impl of this trait is the
glue that drives that encoder. This spec does NOT re-derive the encoder either.

---

## Concept promise served

Same north-star clause the landed seams quote, already verbatim in `kernel/src/arch/mod.rs`:

> "RaeenOS refuses ISA lock-in: the kernel sits on a clean `arch::` abstraction layer (boot, **MMU**,
> interrupts, timers, SMP, context switch, syscall entry, firmware discovery) so the same OS boots
> x86_64, aarch64 (ARM 64-bit), and i686 (32-bit x86) — each proven independently."
> (RaeenOS_Concept.md §Architecture Reach)

The MMU is the load-bearing word in that clause. x86_64 has **one** translation root (`CR3`) and
maps the kernel into the higher half of every address space; aarch64 has **two** (`TTBR1_EL1`
kernel + `TTBR0_EL1` user). The page-table *mechanism* — `OffsetPageTable`/`Mapper`/`Cr3` on x86 vs
raw VMSAv8 descriptor writes on aarch64 — has no common crate; it MUST be hidden behind a
hand-written `arch::mmu` seam. Until that seam exists, every page-table call site is welded to the
`x86_64` crate and no aarch64 backend can provide its translation logic without forking shared MM.

---

## 0. The fan-out reality (grep-quantified — this is the migration size)

Cluster B from the Slice-1 spec is this slice's scope: the paging-TABLE types
`PhysFrame | OffsetPageTable | PageTable | PageTableFlags | Mapper | FrameAllocator | Cr3 |
Page | Size4KiB` → **~319 occurrences across 26 files**, heavily concentrated:

| File | hits | role (audited from source) |
|---|---|---|
| `kernel/src/memory.rs` | 134 | the MM/paging CORE — every function below |
| `kernel/src/posix.rs` | 38 | mmap/mprotect/munmap page-table flag manipulation |
| `kernel/src/syscall.rs` | 35 | mmap/mprotect dispatch + addr math |
| `kernel/src/tpm.rs` | 24 | maps the TPM MMIO window |
| `kernel/src/smp.rs` | 17 | AP trampoline page tables + the **CR3 write on AP bring-up** (1829) |
| `kernel/src/numa.rs` | 11 | NUMA-local page-table allocation |
| `context.rs`, `iommu.rs`, `gpu.rs`, `tpm.rs`, `virtio.rs`, … | ≤5 each | leaf MMIO maps + the **CR3 switch in `switch_context` asm** |

**The true new mechanism lives in `memory.rs`** (134 hits). The other 25 files are *callers* that,
once `memory.rs` exposes the arch-neutral seam, mostly migrate by import rewrite + a thin shim.

### The exact x86 paging surface this slice abstracts (audited — `kernel/src/memory.rs`)

These are the public functions whose bodies name `x86_64` page-table machinery and that the trait
must subsume (line numbers as of this audit):

| fn | line | what it does | trait op it becomes |
|---|---|---|---|
| `active_page_table()` | 87 | `OffsetPageTable` over the **active CR3** | (internal to x86 impl — NOT a trait op; see §7 risk) |
| `kernel_page_table()` | 95 | `OffsetPageTable` over **`KERNEL_PML4`** regardless of CR3 | `AddressSpace::kernel()` |
| `kernel_translate_addr(v)` | 107 | translate via the **kernel** PML4 (safe under user CR3) | `AddressSpace::kernel().translate(v)` |
| `with_kernel_cr3(f)` | 113 | run `f` with CR3 = kernel, restore prior | x86-internal (TTBR1 always-on on aarch64 → no-op) |
| `alloc_kernel_stack(sz)` | 141 | map a guard-paged stack into **`KERNEL_PML4`** | `AddressSpace::kernel().map_range(..)` (+ §10.3 ordering) |
| `create_new_pml4()` | 393 | allocate + clone-kernel-half a new **user** PML4 | `AddressSpace::new_user() -> Root` |
| `map_page_in_pml4(pml4,…)` | 520 | map one page into a **named** PML4 frame | `AddressSpace::from_root(r).map_page(..)` |
| `map_mmio_region(pa,sz)` | 636 | map an MMIO window (Device/NC cache) into kernel | `AddressSpace::kernel().map_mmio_range(..)` |
| `map_phys_ram_into_current_task` | 1099 | map phys RAM (WB) into the **current task's** PML4 | `AddressSpace::current_user().map_range(..)` |
| `phys_to_virt(p)` | 1142 | offset add (direct phys map) | stays a free fn (offset arithmetic, arch-neutral) |
| `virt_to_phys(v)` | 1150 | kernel-then-active translate | `AddressSpace::translate` (kernel + current fallback) |
| `free_kernel_stack` / `free_user_page_tables` | — | `Mapper::unmap` + frame free | `AddressSpace::unmap_range` / `destroy()` |

Plus the **two CR3 writes that are NOT in `memory.rs`** and are the highest-risk part of the whole
slice:
- `kernel/src/context.rs` `switch_context` asm, `rdx = new_cr3`, `mov cr3, rdx` (the per-task
  address-space swap on every context switch — §10.6 area).
- `kernel/src/smp.rs:1829` `Cr3::write(...)` on AP bring-up.

---

## 1. The operations the trait must expose (derived from the table above)

The trait is **small and total** — exactly the operations `memory.rs` actually performs, no more
(minimalism is the architecture, Concept §R7). Naming below is the recommendation; bikeshed-able.

```
arch::mmu  (the seam module — x86 impl backed by OffsetPageTable, aarch64 impl by aarch64_logic)

  // ── A translation-table root handle (opaque per arch) ─────────────────────
  //   x86: a PhysFrame of the PML4.  aarch64: a PhysAddr of the L0 table page,
  //   tagged with which TTBR it belongs to (see §3 divergence).
  type Root: Copy;                          // arch::mmu::Root

  // ── Constructing / selecting an address space ─────────────────────────────
  fn kernel() -> AddressSpace;              // wraps the always-resident kernel root
                                            //   x86: KERNEL_PML4.  aarch64: TTBR1 root.
  fn current_user() -> AddressSpace;        // the running task's USER space
                                            //   x86: active CR3.  aarch64: TTBR0 root.
  fn from_root(r: Root) -> AddressSpace;    // wrap a named root (for map_page_in_pml4 sites)
  fn new_user() -> Result<Root, MmuError>;  // create a fresh USER address space
                                            //   x86: create_new_pml4 (clone kernel half).
                                            //   aarch64: fresh empty L0 (kernel is in TTBR1,
                                            //   NOT cloned — the core divergence, §3).

  // ── Per-AddressSpace operations (the verbs) ───────────────────────────────
  impl AddressSpace {
    fn map_page(&mut self, v: VirtAddr, p: PhysAddr, f: PageFlags) -> Result<(), MmuError>;
    fn map_range(&mut self, v: VirtAddr, p: PhysAddr, len: usize, f: PageFlags)
        -> Result<(), MmuError>;            // contiguous-frame-aware where it allocates tables
    fn unmap_page(&mut self, v: VirtAddr) -> Result<PhysAddr, MmuError>;   // returns freed frame
    fn unmap_range(&mut self, v: VirtAddr, len: usize) -> Result<(), MmuError>;
    fn translate(&self, v: VirtAddr) -> Option<PhysAddr>;
    fn update_flags(&mut self, v: VirtAddr, f: PageFlags) -> Result<(), MmuError>; // mprotect
    fn root(&self) -> Root;                 // for stashing in Task.pml4 / Task.ttbr0
    fn destroy(self);                       // tear down a USER space's private tables
  }

  // ── TLB maintenance (explicit — see §7 risk: silent TLB bugs reboot-loop) ──
  fn flush(v: VirtAddr);                    // x86: invlpg.  aarch64: tlbi vae1is + dsb;isb.
  fn flush_all();                           // x86: reload CR3 / flush_all.  aarch64: tlbi vmalle1is.

  // ── The address-space SWITCH (the §10.6 hot-path keystone) ────────────────
  //   This is the SUBTLE op. It must express "make THIS user space active" without
  //   the x86 impl (one CR3) breaking and without the aarch64 impl touching TTBR1.
  fn user_root_token(r: Root) -> u64;       // the value the context-switch asm loads:
                                            //   x86: the PML4 phys frame addr (-> CR3).
                                            //   aarch64: the TTBR0_EL1 value (table base | ASID).
  // (switch itself stays in arch::context::switch — see §3; this fn produces the token it loads.)
```

### `PageFlags` — the arch-neutral flag set

The pure-logic crux. A single bitflags type the kernel uses everywhere, lowered per-arch:

```
bitflags PageFlags {
  PRESENT     // x86: PRESENT          aarch64: descriptor VALID (bit 0) + AF (bit 10)
  WRITABLE    // x86: WRITABLE         aarch64: AP[2:1] RW vs RO selection
  USER        // x86: USER_ACCESSIBLE  aarch64: AP[2:1] EL0+EL1 vs EL1-only
  NO_EXECUTE  // x86: NO_EXECUTE (NX)  aarch64: UXN|PXN (split below)
  GLOBAL      // x86: GLOBAL           aarch64: nG cleared (global) vs set (per-ASID)
  // cache type is an ENUM field, not a bit (3 device + 2 normal modes don't fit 1 bit):
  cache: CacheType { WriteBack, WriteThrough, Uncached, Device }
}
```

Mapping rules (the host-KAT-able pure logic — extends `aarch64_logic`'s existing KAT suite):

| PageFlags | x86 `PageTableFlags` | aarch64 `LeafAttrs` (via `aarch64_logic::mmu`) |
|---|---|---|
| `PRESENT` | `PRESENT` | `DESC_VALID` + `af=true` |
| `WRITABLE` set, `USER` clear | `WRITABLE` (no USER) | `ap = RwEl1` |
| `WRITABLE` set, `USER` set | `WRITABLE \| USER_ACCESSIBLE` | `ap = RwEl0El1` |
| `WRITABLE` clear, `USER` set | `USER_ACCESSIBLE` (no WRITABLE) | `ap = RoEl0El1` |
| `WRITABLE` clear, `USER` clear | `PRESENT` only | `ap = RoEl1` |
| `NO_EXECUTE`, kernel page | `NO_EXECUTE` | `pxn=true, uxn=true` |
| `NO_EXECUTE`, user page | `NO_EXECUTE` | `uxn=true` (PXN policy: kernel never execs user → `pxn=true`) |
| executable user page | (NX clear) | `uxn=false, pxn=true` |
| `GLOBAL` | `GLOBAL` | `ng=false` |
| `cache=WriteBack` | (no PCD/PWT) | `attr_index=0` (MAIR Normal-WB), `sh=InnerShareable` |
| `cache=Device`/`Uncached` | `NO_CACHE` (PCD) | `attr_index=1` (MAIR Device-nGnRnE), `sh=NonShareable` |

This table IS the `PageFlags → {PageTableFlags, LeafAttrs}` conversion. It is **pure arithmetic**,
FAIL-able, and host-KAT-able on the dev box (§5) BEFORE any boot — the cheapest real proof
(CLAUDE.md §15). The x86 direction is trivial (near-1:1); the aarch64 direction feeds exactly the
`LeafAttrs` struct the existing encoder consumes, so the KAT can assert known
`(PageFlags) → encode_leaf(...)` descriptor words against the values already proven in
`aarch64_logic`'s tests.

**Deliberately NOT in the trait** (keep it minimal): huge-page maps (`HUGE_PAGE` stays an internal
x86 fast-path detail until aarch64 needs 2 MiB blocks — `aarch64_logic` already supports `Block`,
so this is a clean later extension), ASID management (aarch64-only, hidden inside
`user_root_token`), and the direct-phys-map offset (`phys_to_virt` stays a free fn — it is offset
arithmetic, not a page-table walk).

---

## 2. The trait SHAPE — RECOMMENDATION: **module of free fns + a concrete `AddressSpace` struct with `cfg`-selected internals (NOT a Rust `trait`)**

Three shapes weighed against the house tie-breaker (Concept reach > beats-the-alternatives >
simplest-correct-now > reversibility):

### Option (a) — a Rust `trait AddressSpace` with an x86 impl + a future aarch64 impl
A `trait` with `map_page`/`translate`/… and one `impl` per arch.
- **Reach:** full.
- **Cost / against:** the kernel never holds *two* arches' address spaces at once
  (`#[cfg(target_arch)]` picks exactly one), so a `trait` buys zero polymorphism — it only adds
  `<A: AddressSpace>` bounds or `dyn` noise. A `dyn AddressSpace` is **forbidden on the hot path**
  (the `switch`/`translate` paths run per context-switch and per page fault — the Concept latency
  contract and `[BOOT-BENCH]` gate reject a vtable hop, exactly the rule `arch/mod.rs` already
  states). A generic bound threaded through 26 files is the same signature-churn the Slice-1 spec
  rejected for the address types. **Rejected.**

### Option (b) — RECOMMENDED: `arch::mmu` module with free fns + a concrete `struct AddressSpace` whose body is `#[cfg(target_arch)]`-selected
The seam is a **module**, mirroring the four landed seams (`arch::interrupts`, `arch::cpu`,
`arch::interrupt_controller`, `arch::timer`) and the address-type seam (`arch::addr`) exactly.
`arch::mmu::AddressSpace` is **one concrete type** whose private internals differ per arch:

```
// arch/x86_64/mmu.rs
pub struct AddressSpace { root: PhysFrame /* PML4 */, kind: Space }   // backed by OffsetPageTable
// arch/aarch64/mmu.rs  (future — A4)
pub struct AddressSpace { root: PhysAddr  /* L0 */,  ttbr: Ttbr }     // backed by aarch64_logic
```

Every method monomorphizes to the underlying primitive with **zero indirection** — `map_page` on
x86 lowers to today's `map_page_in_pml4_fallible` body; on aarch64 to a descriptor-table walk that
calls `aarch64_logic::encode_leaf`. Same codegen as direct calls today.
- **Reach:** full — gives aarch64 its slot, kills `x86_64::{OffsetPageTable,Mapper,Cr3,…}` from
  shared code.
- **Simplest-correct-now:** strongest — it is the **identical shape** to the five seams already
  landed and verifier-blessed (`arch::<name>` module, `pub fn` + concrete types, no `dyn`, no
  generic bounds). Shared code writes `arch::mmu::AddressSpace::kernel().translate(v)`; the x86
  body is today's code moved behind the name.
- **Reversibility:** high — the x86 impl in 1.5a *delegates to the existing `memory.rs` functions*
  (zero behavior change, §4), so reverting = deleting the seam module and restoring imports.
- **Tie-breaker verdict:** wins on reach (= a), simplest-correct (≫ a, no generic/dyn noise), and
  matches the precedent the architecture-gate already enforces. **RECOMMENDED.**

### Option (c) — a `struct` with raw `enum Backend { X86(..), Aarch64(..) }` runtime tag
A single non-cfg struct holding a runtime-tagged backend.
- **Against:** pays a runtime branch on every map/translate for a choice that is fixed at compile
  time, and compiles *both* backends into every build (aarch64 descriptor code in the x86 kernel) —
  dead mass, Concept §R7 violation. **Rejected.**

**RECOMMENDATION: Option (b).** `arch::mmu` as a seam module exposing free fns
(`kernel()`, `current_user()`, `new_user()`, `flush*`, `user_root_token`) + a concrete
`AddressSpace` struct with `cfg`-selected internals and inherent methods. This is the exact pattern
of the five landed seams — the architecture-gate, the smoketest wiring, and the `/proc/raeen/arch`
reporting all already know this shape.

---

## 3. The kernel-vs-user address-space divergence (the subtle part — spec carefully)

This is the one place where x86 and aarch64 are genuinely *structurally different*, and the trait
must express it without leaking either model.

### The two hardware models
- **x86_64:** ONE root register (`CR3`). The kernel is mapped into the **higher half** of *every*
  address space (`create_new_pml4` clones the kernel PML4 entries into each new user space — audited
  at `memory.rs:393`, lines 421-423 copy all 512 entries, then deep-copy only the low user PD[0]).
  Switching address space = one `mov cr3` (audited: `context.rs` `switch_context` rdx, and
  `smp.rs:1829`). Translating a kernel VA works under *any* CR3 because the kernel half is identical
  everywhere — hence `kernel_translate_addr` reads `KERNEL_PML4` directly (memory.rs:107) and is
  "safe while user CR3 is active" (its docstring).
- **aarch64:** TWO root registers. `TTBR1_EL1` holds the **kernel** tables (high VA, `0xFFFF_…`) and
  is programmed **once at boot and never swapped** — the kernel is *always* resident with no
  per-space cloning. `TTBR0_EL1` holds the **user** tables (low VA) and is swapped per task. The
  high/low split is by VA bit (selected by `TCR_EL1.T1SZ`/`T0SZ`, both 16 → 48-bit halves — the
  exact `tcr_el1_4k_48bit()` the encoder already builds).

### The seam design that makes both correct

The trait expresses **two distinct notions** and never conflates them:

1. **`AddressSpace::kernel()`** — the kernel mappings.
   - x86: an `AddressSpace` whose `root` is `KERNEL_PML4`; `translate` reads it directly (today's
     `kernel_page_table()`).
   - aarch64: an `AddressSpace` whose `root` is the `TTBR1_EL1` table; identical method surface.
   - **Invariant the trait documents:** mapping into `kernel()` is visible in *every* address space.
     On x86 this is true because the kernel half is cloned into every PML4 *and* because
     `alloc_kernel_stack` maps into `KERNEL_PML4` specifically (the source of truth other clones
     copied from). On aarch64 it is true because TTBR1 is shared by construction. **The seam hides
     "how it's shared" — the caller only knows kernel maps are global.**

2. **`AddressSpace::new_user()` / `current_user()`** — the per-task user mappings.
   - x86: `new_user()` = `create_new_pml4` (allocate a PML4 **and clone the kernel half into it** so
     the higher half resolves under this CR3). The cloning is an **x86-impl-internal step**, NOT a
     trait verb — aarch64's `new_user()` allocates an *empty* L0 (no clone; the kernel is in TTBR1).
   - aarch64: `new_user()` = allocate one zeroed L0 table page (4 KiB, via
     `allocate_contiguous_frames` — §10.7), return its PA as the `Root`. No kernel entries.
   - **This is the divergence the §4 spec called out:** the trait must let the caller say "give me a
     fresh user space" WITHOUT the caller knowing whether the kernel half is cloned (x86) or
     ambient (aarch64). `new_user()` does exactly that — the kernel-half handling is buried in each
     arch's impl.

3. **The SWITCH** — "make this user space active."
   - x86: write the user PML4 frame to `CR3` (one register; the kernel half rides along because it's
     cloned into that PML4). This happens in the `switch_context` asm (`mov cr3, rdx`) and the trait
     supplies the *token* via `user_root_token(root)` → the PML4 phys addr that the asm loads into
     rdx. **The asm stays per-arch** (`arch::context::switch`); the trait only computes the token.
   - aarch64: write the user L0 base (+ ASID) to **`TTBR0_EL1` only** — `TTBR1_EL1` is untouched
     (kernel stays resident). `user_root_token(root)` returns the `TTBR0_EL1` value; the aarch64
     `arch::context::switch` does `msr ttbr0_el1, x; tlbi/dsb/isb` for that token instead of
     `mov cr3`.
   - **Why this is clean:** the shared scheduler/context code calls
     `arch::mmu::user_root_token(task.root)` to get the opaque `u64` it stashes in the task and
     hands to `arch::context::switch`. On x86 that `u64` flows to CR3; on aarch64 to TTBR0. **Shared
     code never names CR3 or TTBR — it only knows "the user-root token for this task."** The x86
     impl's "one CR3 carries both halves" and aarch64's "TTBR0 is only the user half" are both
     satisfied because the token is *defined per arch* to be "whatever the switch must load to make
     this user space active." The x86 impl does NOT break (its token is the full PML4, which is what
     CR3 wants); the aarch64 impl expresses "switch the USER half" natively (TTBR0 only).

### The one place to be careful (and the spec's instruction to the implementer)
`kernel_translate_addr` (memory.rs:107) MUST remain backed by `AddressSpace::kernel()` and NEVER by
`current_user()`. On x86 this is the §10.2 keystone ("translate a kernel VA using the kernel PML4,
safe while user CR3 is active"); on aarch64 it is automatic (kernel VAs resolve through TTBR1
regardless of TTBR0). The trait makes the correct call the obvious one: shared kernel code that
wants a *kernel* VA translated calls `arch::mmu::AddressSpace::kernel().translate()`; only user-VA
translation goes through `current_user()`. **Migrating `virt_to_phys` (memory.rs:1150 — kernel-then-
active fallback) must preserve this kernel-first order** (§7 risk).

---

## 4. Incremental migration strategy (CRITICAL — boot-critical; mirror the Slice-1 / seam discipline)

The page tables are boot-critical: a wrong move = no boot. The migration mirrors the
seam-relocation and Slice-1 discipline — **small, individually-verifiable steps, x86 stays 7/7 at
every step, zero behavior change until proven**. The key trick (same as Slice 1a): **introduce the
seam as a DELEGATING wrapper first**, so the first commit changes *no behavior at all*, then move
call sites subsystem-by-subsystem, then (last) reimplement the seam's body to stop delegating.

Baseline proof line for EVERY sub-slice below (the falsifiable gate):
> `[ OS ] System successfully booted.` + `boot health: 6/6 critical PASS -> HEALTHY`, **no
> `[PANIC]`**, `[BOOT-BENCH]` not regressed, and the **`arch` smoketest line still prints `-> PASS`**
> (now also reporting the `mmu=` token, §5). SMP/CR3-touching sub-slices get **≥5 boots at
> `RAEEN_SMP=1` and `=2`** (CLAUDE.md §17) — flagged ⚠ below.

### Sub-slice 1.5a — define `arch::mmu` + `PageFlags` + an x86 impl that DELEGATES to existing `memory.rs`, + a map/translate/unmap smoketest (ZERO behavior change) — **THE NAMED HAND-OFF (§8)**
- **Files:**
  - `kernel/src/arch/x86_64/mmu.rs` (**NEW**): `PageFlags` bitflags + `CacheType`; `MmuError`;
    `struct AddressSpace { root, kind }`; `Root = PhysFrame`; the free fns + inherent methods —
    **each body calls the EXISTING `crate::memory` function** (`kernel()` → wraps `KERNEL_PML4`;
    `new_user()` → `create_new_pml4()`; `map_page` → `map_page_in_pml4_fallible`; `translate` →
    `kernel_translate_addr` / active; `flush` → `invlpg`; `user_root_token` → the PML4 frame addr).
    `PageFlags::to_x86()` does the §1 table's x86 column. **No `memory.rs` body changes yet.**
    R10 docstring quoting §Architecture-Reach.
  - `kernel/src/arch/x86_64/mod.rs`: `pub mod mmu; pub use mmu::*;`
  - `kernel/src/arch/mod.rs`: extend `run_boot_smoketest()` with the `arch::mmu` round-trip
    (§5) + append `mmu=…` to the smoketest line; move `paging-tables` from `seams_pending` to
    `seams_online` in `dump_text()`.
- **Why first:** pure *addition* + a delegating wrapper — nothing in `memory.rs` is rewritten, so
  the proof is "x86 STILL 7/7 + the new `mmu=roundtrip-ok` token prints PASS." Exactly how Slice 1a
  and the four seams opened (define the predicate + smoketest before any dependent edit). **Zero
  MM-core risk.**
- **Proof line:** baseline + `mmu=roundtrip-ok` in the `[arch] smoketest:` line. Host-KAT the
  `PageFlags` conversion first (§5).

### Sub-slice 1.5b — migrate LEAF MMIO-mapping callers (lowest fan-out, no MM-core coupling)
- **Files (≤5 hits each, leaf `map_mmio_region`/single-page consumers):** `tpm.rs` (24 — but all
  one MMIO window), `virtio.rs`, `gpu.rs`, `iommu.rs` (its own IOVA tables are separate — only its
  *kernel* MMIO maps migrate), `xhci.rs`, `compositor.rs` framebuffer map. Rewrite to
  `arch::mmu::AddressSpace::kernel().map_mmio_range(pa, len, PageFlags::DEVICE)`.
- **Why second:** isolated, non-SMP, non-CR3 — proves the seam handles the MMIO/Device cache path
  before touching hot files.
- **Proof line:** baseline (these subsystems' own smoketest markers unchanged).

### Sub-slice 1.5c — migrate the `map_page_in_pml4` / named-root callers
- **Files:** the sites that map into a *named* PML4 (ELF loader page-in, `map_phys_ram_into_current_task`
  at memory.rs:1099). Rewrite to `AddressSpace::from_root(r)` / `current_user().map_range(..)`.
- **Proof line:** baseline.

### Sub-slice 1.5d — migrate `mmap`/`mprotect`/`munmap` (`syscall.rs` + `posix.rs`) — flag mgmt
- **Files:** `posix.rs` (38), `syscall.rs` (35). These do `PageTableFlags` manipulation for
  user mappings → migrate to `PageFlags` + `AddressSpace::current_user().{map_range, update_flags,
  unmap_range}`. **`update_flags` is the mprotect path** — verify R/W/X transitions per the §1
  table.
- **CONCURRENCY GATE:** if `sys_mprotect`-area work is in flight, sequence AFTER it merges
  (coordinate via the lead) — same gate the Slice-1 spec applied to these files.
- **Proof line:** baseline + a user mmap/mprotect smoketest marker if one exists; else the daemon
  chain (which mmaps) reaching `System successfully booted`.

### Sub-slice 1.5e — migrate `alloc_kernel_stack` / `free_kernel_stack` (kernel-space maps) ⚠
- **Files:** `memory.rs` `alloc_kernel_stack` (141), `free_kernel_stack` (174). These map into
  `KERNEL_PML4` → `AddressSpace::kernel().map_range(.., PageFlags::PRESENT|WRITABLE|NO_EXECUTE)`.
- **§10.3 KEYSTONE:** the docstring's "call this **before** `create_new_pml4()`" ordering MUST be
  preserved — the migration is a body rewrite that keeps the same call order. The seam does NOT
  change *when* the stack is allocated relative to `new_user()`.
- **Proof line:** baseline **+ ≥5 boots `RAEEN_SMP=1`/`=2`** (every spawned task gets a kernel
  stack; corruption here = the worst-bug class, §10.6).

### Sub-slice 1.5f — migrate `create_new_pml4` → `AddressSpace::new_user()` ⚠ (HIGHEST RISK)
- **Files:** `memory.rs:393` `create_new_pml4`. Reimplement the seam's `new_user()` body to BE the
  kernel-half-cloning logic (stop delegating); update the spawn path (`elf.rs`/`task.rs`) to call
  `arch::mmu::new_user()` and stash the returned `Root` in `Task.pml4`.
- **§10.3 KEYSTONE:** the per-task kernel stack MUST still be allocated BEFORE `new_user()` so the
  clone captures the stack mapping (the audited reason at memory.rs:138-140). The migration must NOT
  reorder these. The deep-copy of user low PD[0] (the frame-collision fix audited at memory.rs:470-
  490 — "child mapping its text at base 0 REPLACES the parent's pages") MUST be preserved verbatim
  in the x86 `new_user()` impl.
- **Proof line:** baseline **+ ≥5 boots `RAEEN_SMP=1`/`=2`** + the spawn-and-reap daemon chain
  proven (user_init + daemons spawn without #UD — the regression class this code fixed).

### Sub-slice 1.5g — migrate the CR3 SWITCH → `arch::mmu::user_root_token` ⚠⚠ (HIGHEST RISK — §10.6)
- **Files:** `context.rs` (the `switch_context` caller that computes `new_cr3`), `smp.rs:1829`,
  `scheduler.rs` (where the per-task root is read for the switch). Replace the raw "task PML4 frame
  → CR3" computation with `arch::mmu::user_root_token(task.root)`; the value still flows to
  `switch_context`'s rdx on x86 (and the asm `mov cr3, rdx` is UNCHANGED — only how rdx is *computed*
  moves behind the seam). On aarch64 (A4/A9) the same token flows to `msr ttbr0_el1`.
- **§10.6 KEYSTONE:** this touches the address-space swap on EVERY context switch + AP bring-up —
  the single most boot-critical edit in the slice. Pure token-computation relocation ONLY; do NOT
  alter the `mov cr3` asm, the block-path syscall-stack handling, or the lock-drop-before-switch
  ordering.
- **Proof line:** baseline **+ ≥5 boots `RAEEN_SMP=1`/`=2`** (the steal-resume race class lives
  exactly here; CLAUDE.md §17 + pitfall #9 — don't trust 2 green boots).

### Sub-slice 1.5h — reimplement the leaf map/unmap/translate seam bodies (stop delegating) + the `memory.rs` Cluster-B cleanup ⚠
- **Files:** `memory.rs` — flip `map_page_in_pml4_fallible`, `kernel_page_table`,
  `kernel_translate_addr`, `map_mmio_region`, `virt_to_phys`, `free_user_page_tables` to live
  INSIDE `arch::x86_64::mmu` (the seam now owns the `OffsetPageTable`/`Mapper`/`Cr3` machinery; the
  thin `memory.rs` wrappers become 1-line forwards or are deleted). After this, **no shared kernel
  file names `x86_64::{OffsetPageTable, Mapper, Cr3, PageTableFlags, PhysFrame, Page, Size4KiB}`**
  — only `arch/x86_64/mmu.rs` does.
- **Proof line:** baseline **+ ≥5 boots `RAEEN_SMP=1`/`=2`**.

### Completion check for Slice 1.5
After 1.5a–1.5h: grep for `x86_64::structures::paging` / `x86_64::registers::control::Cr3` in
shared kernel files returns **only `kernel/src/arch/x86_64/mmu.rs`** (+ `context.rs`/`smp.rs` asm
that the `arch::context` seam owns). That grep is the falsifiable "Slice 1.5 done" gate. The
aarch64 backend (A4) can now plug a second `arch::mmu` impl into the identical seam.

---

## 5. R10 + proof — the FAIL-able smoketest + host-KATs

### The boot smoketest (extends `arch::run_boot_smoketest()`, sub-slice 1.5a)
A **live page-table round-trip through the seam** (the FAIL-able R10 artifact):
1. Allocate one frame (`allocate_contiguous_frames(0)`).
2. Pick an unused kernel test VA (high in the `KERNEL_STACK_ALLOCATOR` range or a dedicated probe
   VA). `AddressSpace::kernel().map_page(test_va, frame_pa, PageFlags::PRESENT|WRITABLE|NO_EXECUTE)`.
3. `AddressSpace::kernel().translate(test_va)` → assert it equals `frame_pa`. **FAIL-able:** a wrong
   flag lowering, a broken table walk, or a missing TLB flush prints `mmu=ROUNDTRIP-BAD -> FAIL`.
4. Write a sentinel through `test_va`, read it back, assert (proves the mapping is *live*, not just
   present in the table).
5. `unmap_page(test_va)`; assert `translate(test_va)` is now `None` (proves unmap + TLB flush).
   **FAIL-able:** a no-op unmap or missing `invlpg` leaves it mapped → FAIL.
6. Free the frame.

Smoketest line gains an `mmu=` token (extending the current
`… addr=roundtrip-ok -> PASS` line):
```
[arch] smoketest: name=x86_64 … addr=roundtrip-ok mmu=roundtrip-ok -> PASS
```
On any failure: `… mmu=ROUNDTRIP-BAD -> FAIL`. This runs AFTER `memory::init` (it needs a live
allocator + `PHYS_MEM_OFFSET`), so the smoketest call-site moves to (or a second mmu-specific
smoketest is added at) a post-`memory::init` boot tier — note this for the implementer; the
existing arch smoketest runs early (pre-`memory::init`), so the `mmu=` assertion is a SEPARATE,
later smoketest emission, not folded into the early one.

### Host-KATs (pure logic — cheapest proof, do FIRST per CLAUDE.md §15)
- **`PageFlags → PageTableFlags`** (x86 column of §1): assert each row. FAIL-able.
- **`PageFlags → LeafAttrs → encode_leaf(...)`** (aarch64 column): assert the produced descriptor
  word equals the known-good value — this **extends `components/aarch64_logic`'s existing KAT
  suite** (the `block_2mib_l2_known_value` / `page_4kib_l3_user_rw_exec` tests already prove the
  encoder; the new KATs prove the `PageFlags`→`LeafAttrs` mapping that feeds it). Run with
  `cargo test -p aarch64_logic`. This is where the aarch64 impl is de-risked *before any aarch64
  boot exists* — exactly how the x86 ACPI bug was root-caused off-target.
- **Level-index arithmetic** (VA → L0/L1/L2/L3 indices for the 48-bit/4 KiB walk): pure, KAT-able;
  belongs in `aarch64_logic` alongside the encoder.

---

## 6. Interface note — internal kernel HAL, NOT `rae_abi`

**Confirmed: `ABI_VERSION` is UNCHANGED.** `arch::mmu::{AddressSpace, PageFlags, Root, MmuError}`
are **internal kernel HAL types** — they never cross the syscall boundary:
- mmap/mprotect/brk syscalls keep their ABI-level args as **plain `u64`/`usize` integers**
  (`rae_abi` is untouched). The handler constructs `PageFlags` from the raw `prot` bits *inside* the
  kernel and calls `current_user().update_flags(..)`; the `arch::mmu` types never appear in a
  `rae_abi` constant or struct. This is already true today (`PageTableFlags` is built inside
  handlers, not in `rae_abi`) — the migration preserves it.
- No `rae_abi` / `rae_driver_api` edit. The architecture-gate's `[interface]` sign-off requirement
  does NOT apply (it gates `rae_abi`/`rae_driver_api`, not internal `arch::`).
- The widened `arch::mmu` surface is the internal seam contract **raeen-architect** owns as
  documentation — consistent with the five landed seams. No `[interface]` commit tag needed (that
  tag is `rae_abi`-only); these land as ordinary kernel commits under `RAEEN_AGENT=opus`.

---

## 7. Risk register — what breaks the boot, and how the slices de-risk it

| # | Risk (what breaks boot if done wrong) | Source | De-risked by |
|---|---|---|---|
| R1 | **Active-CR3 vs `KERNEL_PML4` confusion** — translating a kernel VA through the *active user* CR3, or mapping a kernel stack into the active CR3 instead of `KERNEL_PML4`, gives wrong/absent mappings → #PF reboot loop. | §10.2; memory.rs:94-110 | `AddressSpace::kernel()` is a DISTINCT handle from `current_user()`; `kernel_translate_addr` stays backed by `kernel()` (1.5h preserves the kernel-first order in `virt_to_phys`). The smoketest (§5) maps+translates through `kernel()` explicitly. |
| R2 | **Per-task kernel stack allocated AFTER `new_user()`** — the clone misses the stack mapping → the task faults the instant it touches its stack → #DF. | §10.3; memory.rs:138-140 | 1.5e (stack maps) lands BEFORE 1.5f (`new_user`); both spec the ordering as inviolable; the x86 `new_user()` impl keeps the clone *after* the stack alloc, verbatim. |
| R3 | **TLB not flushed on map/unmap/switch** — stale TLB entry → reads/writes hit the wrong (or freed) frame; classic silent corruption that boots "sometimes." | §10 general; memory.rs unmap paths | `flush(v)`/`flush_all()` are EXPLICIT trait ops (not implicit in map/unmap), and the §5 smoketest step 5 asserts `translate` returns `None` after unmap — a missing flush prints FAIL. The CR3-switch (1.5g) keeps the existing flush-on-reload semantics. |
| R4 | **CR3-switch token miscomputed** (1.5g) — wrong PML4 → task runs in the wrong address space → instant fault or, worse, silent cross-task corruption (the steal-resume race class). | §10.6; context.rs, smp.rs:1829 | 1.5g is pure *token-computation* relocation; the `mov cr3, rdx` asm is UNCHANGED; ≥5 boots at `RAEEN_SMP=1`/`=2` (pitfall #9 — work-stealing race lives here; don't trust 2 green boots). |
| R5 | **Frame-collision regression** — losing the user-low-PD[0] deep-copy in `new_user()` → child's base-0 text REPLACES the running parent's pages → parent executes child bytes → #UD (the audited 2026-06-10 bug). | memory.rs:470-490 | 1.5f preserves the deep-copy verbatim; proof line requires the spawn-and-reap daemon chain green (the exact scenario that regressed). |
| R6 | **Multi-page table allocation non-contiguous** — allocating page-table pages (esp. aarch64's L0-L3 build) with an `allocate_frame()` loop instead of `allocate_contiguous_frames(order)` → walker reads garbage from a non-contiguous "table" → wild faults three subsystems away. | §10.7; pitfall #7 | The trait's `map_range`/`new_user` spec MANDATES `allocate_contiguous_frames` for any multi-page table region; called out per-slice (1.5e/1.5f). aarch64 A4 inherits the mandate. |
| R7 | **`PageFlags` lowering wrong** — a mis-mapped flag (e.g. NX dropped, or USER set on a kernel page) → either a security hole or an instant fault. | §1 table | Host-KAT'd FIRST (§5) — every §1 row asserted on the dev box before boot; the aarch64 column reuses `aarch64_logic`'s proven encoder. |
| R8 | **Smoketest can't FAIL** — a round-trip that always passes is a false green. | CLAUDE.md §16 | §5 step 5 asserts the *negative* (unmapped → `None`); the host-KAT includes FAIL-demo cases (wrong-flag, wrong-OA) mirroring `aarch64_logic`'s existing `faildemo_*` tests. |

**Net de-risk philosophy:** the delegating-wrapper-first approach (1.5a) means the seam EXISTS and
is proven (smoketest green) before a single `memory.rs` body moves; then each body-move (1.5e–1.5h)
is a contained, individually-bootable step with the highest-risk three (1.5e/f/g) gated on ≥5-boot
SMP runs. No giant atomic `memory.rs` diff ever happens.

---

## 8. HAND-OFF — the named first sub-slice for raeen-kernel

**Execute FIRST: Sub-slice 1.5a — define `arch::mmu` + `PageFlags` + an x86 `AddressSpace` impl that
DELEGATES to the existing `memory.rs` functions, plus a map/translate/unmap smoketest. ZERO behavior
change.**

- **Why this one:** it is the smallest immediately-verifiable step — pure *addition* + a delegating
  wrapper, **no `memory.rs` body rewritten**, so the proof is "x86 STILL 7/7 and the new
  `mmu=roundtrip-ok` token prints PASS." It mirrors exactly how Slice 1a and the four landed seams
  opened (define the seam + smoketest before any dependent edit). It carries zero MM-core risk and
  does not touch the concurrently-dirty `mprotect` lane.
- **Files:**
  - `kernel/src/arch/x86_64/mmu.rs` — **NEW**: `PageFlags` (bitflags) + `CacheType` enum +
    `PageFlags::to_x86() -> x86_64::structures::paging::PageTableFlags` (the §1 x86 column);
    `MmuError`; `pub struct AddressSpace { root: PhysFrame, kind: Space }`;
    `pub type Root = x86_64::structures::paging::PhysFrame;` the free fns (`kernel`, `current_user`,
    `from_root`, `new_user`, `flush`, `flush_all`, `user_root_token`) + the inherent methods
    (`map_page`, `map_range`, `unmap_page`, `unmap_range`, `translate`, `update_flags`, `root`,
    `destroy`) — **each body forwarding to the existing `crate::memory::*` function** (delegation,
    not reimplementation). R10 docstring quoting the §Architecture-Reach clause.
  - `kernel/src/arch/x86_64/mod.rs` — `pub mod mmu; pub use mmu::*;`
  - `kernel/src/arch/mod.rs` — add the post-`memory::init` `arch::mmu` round-trip smoketest (§5),
    append the `mmu=` token to the smoketest line, move `paging-tables` from `seams_pending` to
    `seams_online` in `dump_text()`.
  - (host-KAT, dev-box only) extend `components/aarch64_logic`'s tests OR add a kernel `#[cfg(test)]`
    module asserting the `PageFlags::to_x86()` rows — pure logic, run before boot.
- **Exact boot-log proof line** (the falsifiable gate):
  ```
  [arch] smoketest: name=x86_64 ptr=64 if-save/restore=ok vectors=installed gdt=loaded eoi=exercised timer=exercised(pre-calib) addr=roundtrip-ok -> PASS
  [arch-mmu] smoketest: map+translate+write+unmap through arch::mmu::AddressSpace -> mmu=roundtrip-ok PASS
  ```
  plus the unchanged `[ OS ] System successfully booted.` + `boot health: 6/6 critical PASS ->
  HEALTHY`, no `[PANIC]`, `[BOOT-BENCH]` not regressed. Host-KAT the `PageFlags` lowering on the dev
  box FIRST (`cargo test -p aarch64_logic` for the aarch64 column), then QEMU boot.
- **Then 1.5b → 1.5h** per §4, with **1.5d (`mprotect` lane) sequenced after any in-flight
  `sys_mprotect` work merges**, and **1.5e / 1.5f / 1.5g gated on ≥5 boots at `RAEEN_SMP=1` and
  `=2`** (the §10.3/§10.6 keystones). **No aarch64 `arch::mmu` impl until 1.5h lands** — the seam
  contract must be complete + x86-proven before A4 fills the second backend.

---
Sources:
- `docs/research/slice1-arch-neutral-mm-newtypes.md` §4 (deferred the paging trait to this slice)
- `docs/research/aarch64-bringup-spec.md` §"Design — the arch:: surface" + Slice A4 (the aarch64 MMU
  this trait plugs into) + the two-TTBR insight
- `kernel/src/memory.rs` (audited: `active_page_table`/`kernel_page_table`/`kernel_translate_addr`/
  `with_kernel_cr3`/`alloc_kernel_stack`/`create_new_pml4`/`map_page_in_pml4(_fallible)`/
  `map_mmio_region`/`map_phys_ram_into_current_task`/`phys_to_virt`/`virt_to_phys`)
- `kernel/src/context.rs` (`switch_context` asm — `mov cr3, rdx`) + `kernel/src/smp.rs:1829`
  (`Cr3::write` on AP bring-up)
- `components/aarch64_logic/src/mmu.rs` (the host-KAT'd VMSAv8 descriptor encoder + `TCR_EL1`/
  `MAIR_EL1` builders the aarch64 impl drives)
- `kernel/src/arch/mod.rs` (the five landed seams' module pattern + R10 smoketest/dump_text wiring)
- CLAUDE.md §10.2/§10.3/§10.6/§10.7 (the paging keystones) + §15/§16/§17 (proof ladder, FAIL-able,
  ≥5-boot SMP)
