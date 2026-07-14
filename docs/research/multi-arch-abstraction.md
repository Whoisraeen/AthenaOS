# Spec: Multi-architecture support (arch:: abstraction + bring-up plan)

Status: RESEARCH / ADR-candidate. Read-only spec — no crate modified by this document.
Goal served: owner goal criterion #3 — "AthenaOS must boot, run, and install on x86_64
(current), aarch64 (ARM 64-bit), AND i686 (32-bit x86), each proven independently in QEMU."

## Concept promise served

The Concept doc has **no explicit multi-arch clause** (verified: no `aarch64`/`arm64`/`32-bit`/
`portab*` token in `LEGACY_GAMING_CONCEPT.md`). The closest load-bearing line is the manifesto:

> "macOS got locked behind a walled garden of Apple silicon. ... AthenaOS is the third path —
> a from-scratch, embodiment-first, native-feeling OS that treats power users like adults"
> (LEGACY_GAMING_CONCEPT.md §The OS Manifesto)

Multi-arch is the *anti-"locked behind Apple silicon"* property: the OS is not welded to one
ISA the way macOS is welded to Apple silicon. This is an **owner goal criterion that extends
the Concept's spirit**, not a Concept contract. Per house rules (Concept > checklist > owner
directive on *conflict*), there is no conflict here — the Concept is silent, so the owner
criterion governs. **Recommendation to the lead:** add a one-line multi-arch clause to the
Concept (§Architecture) so future agents have a North-Star quote; this spec proceeds on the
owner criterion in the meantime.

## Already in the tree (verify-before-implement)

There is **no `arch::` boundary today.** The kernel is monolithically x86_64. Survey results:

- **No `kernel/src/arch/` directory exists.** (verified)
- **No `aarch64` / `i686` / `riscv` token anywhere in `kernel/src`.** (verified) Greenfield
  for the other two arches.
- **Boot entry is x86-only:** `kernel/src/main.rs:315` uses `bootloader_api 0.11`'s
  `entry_point!(kernel_main, ...)`. The `bootloader`/`bootloader_api` crate is an
  **x86_64-only** project (BIOS+UEFI on x86; no aarch64 target). This crate **cannot be the
  boot path for aarch64** — that arch needs its own entry (UEFI-direct or the AArch64 Linux
  boot protocol on `qemu virt`). Status of x86_64 boot entry: `[x]` (iron-proven).
- **x86_64-crate coupling is deep and pervasive:**
  - `31` files `use x86_64`; `385` references to `x86_64::` paths across `kernel/src`.
  - `167` inline-asm sites (`core::arch::asm!`/`naked_asm!`/`global_asm!`).
  - `13` files do port I/O via `x86_64::instructions::port::*`.
  - The MM is the worst-coupled: `kernel/src/memory.rs` threads `x86_64`'s `VirtAddr`,
    `PhysAddr`, `OffsetPageTable`, `PageTable`, `PageTableFlags`, `PhysFrame`, `Cr3` through
    its public surface. These newtypes leak into callers across the tree.
- **Genuinely arch-specific modules (the seam set), with current status (all `[x]` x86_64):**
  | file | LOC | what it is |
  |---|---|---|
  | `context.rs` | 111 | `switch_context` global_asm (rbx/rbp/r12-15 + fxsave64 + cr3); user/kernel thread-entry trampolines (iretq, 0x23/0x2B segments) |
  | `gdt.rs` | 260 | GDT + TSS + IST (x86-only concept; aarch64/i686 differ) |
  | `interrupts.rs` | 1545 | IDT, exception handlers, `#GP` MSR-recovery, PIC 8259 |
  | `apic.rs` | 677 | x2APIC/LAPIC, TSC calibration (`TSC_FREQ_MHZ`) |
  | `smp.rs` + `smp/trampoline.asm` | 2521 | AP bring-up via INIT-SIPI-SIPI real-mode trampoline |
  | `msr.rs` / `msr_amd.rs` / `msr_intel.rs` | 186+ | rdmsr/wrmsr + `#GP`-recovered probing (x86-only) |
  | `cpu_features.rs` | 803 | CPUID fingerprint + feature flags (x86-only instruction) |
  | `hpet.rs` / `timers.rs` | 270/1401 | HPET + LAPIC-timer time source |
  | `mmio.rs` | 1547 | MMIO + port I/O helpers |
  | `syscall.rs` | 3296 | dispatch + `syscall`/`sysret` MSR setup (entry is arch; **handlers are shared**) |
  | `memory.rs` + `memory/{buddy,allocator}.rs` | 1302+ | paging (arch) + buddy/heap (shared logic, arch types leak in) |
  | `tpm.rs`, `secure_boot.rs`, `efi.rs`, `smbios.rs`, `acpi*.rs` | — | firmware discovery — **x86-PC-firmware-shaped**; aarch64 uses DeviceTree, not ACPI-MADT/legacy |

- **Shared, already arch-clean (no change needed):** scheduler logic (`scheduler.rs` — only the
  `switch_context` call + per-CPU access are arch), IPC (`ipc.rs`), VFS (`vfs.rs`), AthFS
  (`athfs.rs`), capability (`capability.rs`), all of `components/*` (the Rae* userspace), the
  net stack above the NIC drivers, crypto. These are pure `no_std`+`alloc` logic.

**Delta to design:** everything. There is no abstraction to extend — this spec defines a new
`arch::` boundary from zero, then the order to fill it.

## Prior art & OSS verdict

- **Redox OS kernel** — the closest model. Factors `src/arch/{x86_64,x86(i586),aarch64,riscv64}`
  selected by `#[cfg(target_arch)]`, with a common high-level kernel calling into per-arch
  modules: `start`, `paging`, `interrupt`, `context/switch`, `device`, `ipi`, `time`. Their
  AArch64 paging abstraction is the key lesson: **x86_64 has one `cr3`; aarch64 has two table
  base registers (`TTBR0_EL1` user / `TTBR1_EL1` kernel)** — they widened the paging trait to
  carry "which table root" rather than assume a single root. Boot on aarch64 = an A64 asm stub
  that sets up the MMU from scratch + DTB parse + GICv2 + ARM generic timer + PSCI for SMP.
  Verdict: **📖 study/isolate — Redox is MIT but `redox_reference/` is harvest-only (CLAUDE.md
  §10.13: never push upstream).** Harvest the *layering pattern and the two-TTBR insight*, not
  the code. (`redox_reference/` is **not present locally** — fetch read-only from upstream when
  implementing.)
- **seL4 / Zircon** — both isolate arch behind a `plat`/`arch` layer with a fixed internal
  contract (boot, MMU, IRQ controller, timer, context, SMP). Zircon's `arch::` + `platform::`
  split (CPU-ISA vs board) is worth borrowing: **ISA seam ≠ board/firmware seam.** On x86 the
  board is "PC+ACPI"; on aarch64 it's "DeviceTree". Verdict: **📖 pattern only** (GPL/odd
  licenses; never vendor).
- **Linux `arch/`** — the canonical per-arch tree, but the explicit no-clone rule (CLAUDE.md
  §4.2) bars transplanting its architecture. **Harvest the seam taxonomy, nothing else.**
- **`bootloader` / `bootloader_api` crate (rust-osdev)** — currently in use for x86 boot.
  **x86_64-only.** Verdict: **in-use for x86; NOT usable for aarch64.** aarch64 boot must use a
  different entry (the `uefi` crate works on aarch64, or the AArch64 Linux boot protocol under
  `qemu virt` + `-kernel`). i686 *may* be served by the same `bootloader` crate family (it
  supports BIOS on 32-bit-capable x86) — **verify against the crate at implementation time;**
  if not, a thin custom 32-bit stub is the fallback. ➕ the `uefi` crate is MIT/Apache
  (vendorable) for the aarch64 entry.
- **`gimli`/`object`/DTB parsers (`fdt` crate)** — for aarch64 DeviceTree parsing (replaces the
  x86 ACPI path). `fdt` crate is MIT/Apache. Verdict: **➕ vendorable** when aarch64 reaches
  device discovery.

Respecting Concept §R7 (no Linux-clone lineage): we copy the *idea* of a per-arch module tree
(universal in OS design, not Linux-specific) and Redox's two-TTBR paging insight. No
architecture is transplanted.

## Design

### 1. The arch-abstraction boundary

A new crate-internal module `kernel/src/arch/` with this shape:

```
kernel/src/arch/
  mod.rs              // pub use the active arch via cfg; declares the arch contract
  x86_64/             // existing code moved here, behavior-identical
    mod.rs  boot.rs  cpu.rs  mmu.rs  irq.rs  time.rs  smp.rs  context.rs  syscall_entry.rs  io.rs  percpu.rs
  aarch64/            // (phase 2 — empty stubs until then)
  i686/               // (phase 3)
```

`arch::mod.rs` re-exports the active backend:
```rust
#[cfg(target_arch = "x86_64")] mod x86_64; #[cfg(target_arch = "x86_64")] pub use x86_64::*;
#[cfg(target_arch = "aarch64")] mod aarch64; #[cfg(target_arch = "aarch64")] pub use aarch64::*;
#[cfg(target_arch = "x86")]     mod i686;    #[cfg(target_arch = "x86")]     pub use i686::*;
```

**Boundary style: free functions + a small number of `const`/newtypes, NOT a giant runtime
trait object.** A trait gives a clean *contract for review* but a `dyn Arch` adds a vtable hop
to the hot context-switch path, which violates "hot path is allocation/indirection-free"
(CLAUDE.md heuristic 2). So:
- The **contract** is expressed as a documented set of required `pub fn` signatures +
  associated types that every `arch/<name>/` module MUST provide (enforced at compile time:
  the shared kernel calls `arch::foo()` and won't link if an arch omits it). This is the
  `NEEDS-INTERFACE` artifact `athena-architect` owns as a written internal contract.
- Where a trait *is* warranted (e.g. an `AddressSpace` handle, a `Frame`/`Page` type), it's a
  **type-parameter / associated-type** resolved at compile time per arch — zero runtime cost.

### The seam list (what `arch::` MUST provide; what stays shared)

| Seam | x86_64 today | aarch64 | i686 | arch:: surface (proposed) |
|---|---|---|---|---|
| **Boot entry / early init** | `bootloader_api` `entry_point!` → `kernel_main(&mut BootInfo)` | UEFI-direct or A64 stub + DTB on `qemu virt` | `bootloader` BIOS or 32-bit stub | `arch::boot::_start` → normalizes to a **shared `BootContext`** (memmap, fb, rsdp/dtb ptr, cmdline) → calls shared `kernel_main(BootContext)` |
| **CPU feature init** | CPUID (`cpu_features.rs`) | `ID_AA64*` system regs | CPUID | `arch::cpu::init_features() -> CpuFeatures` (shared `CpuFeatures` struct; arch fills it) |
| **MMU / paging** | 4-level, `Cr3`, `OffsetPageTable` (x86_64 crate) | 4-level, `TTBR0/1_EL1` (two roots) | **PAE** (3-level, 64-bit PTE) or 2-level | `arch::mmu::{AddressSpace, map, unmap, translate, switch_to, kernel_root}`; **own `PhysAddr`/`VirtAddr`/`Frame`/`Page` newtypes in `arch` (de-x86_64-crate the MM public surface)** — biggest refactor |
| **Interrupts** | IDT + exception handlers + APIC EOI + PIC | **exception vectors (VBAR_EL1)** + GICv2/v3 distributor/redistributor | IDT + legacy PIC / APIC | `arch::irq::{init, register_handler, eoi, enable, disable, send_ipi}`; shared handler *logic* (page-fault policy, timer tick) lives in kernel, arch provides the trap frame |
| **Timers** | HPET + LAPIC timer + TSC calib | ARM **generic timer** (`CNTP_*`, PPI) | HPET/PIT + LAPIC | `arch::time::{now_ns, set_oneshot, calibrate, tick_hz}` (shared `MonotonicClock` facade) |
| **SMP bring-up** | INIT-SIPI-SIPI real-mode trampoline | **PSCI `CPU_ON`** | INIT-SIPI-SIPI | `arch::smp::{cpu_count, start_ap(id, entry), this_cpu_id}` |
| **Context switch / task state** | `switch_context` asm (callee-saved + fxsave64 + cr3) | x19-x30/sp + FP/SIMD + TTBR0 | 32-bit GPR set + fxsave + cr3 | `arch::context::{switch, new_kernel_thread, new_user_thread, TaskContext}`; scheduler stays shared and calls `arch::context::switch` |
| **Syscall entry** | `syscall`/`sysret` via `LSTAR`/`STAR`/`SFMASK` MSRs | **`svc #0`** → exception vector | `int 0x80` / `sysenter` | `arch::syscall::init_entry()` + arch asm stub that marshals regs → calls **shared `syscall::dispatch(nr,args)`** (the 3296-line dispatch is **arch-neutral and stays put**) |
| **Per-CPU data** | `GS_BASE` MSR | `TPIDR_EL1` | `GS`/`FS` seg | `arch::percpu::{set_base, this_cpu_ptr}` |
| **I/O** | port I/O (`in`/`out`) + MMIO | **MMIO only** (no port space) | port I/O + MMIO | `arch::io::{inb/outb (x86 only — cfg'd out elsewhere), mmio_read/write}`; callers that assume ports become MMIO-or-error on aarch64 |
| **Firmware discovery** | ACPI (RSDP/MADT/DSDT), SMBIOS, EFI | **DeviceTree (DTB)** | ACPI | `arch::firmware::{discover() -> PlatformTopology}` — abstracts "how do I find CPUs/IRQs/devices" (ACPI-MADT vs DTB). Big secondary seam. |
| **TPM / secure boot** | TPM 2.0 (TIS/CRB), UEFI secure boot | TPM via DTB or fTPM; UEFI | TPM/TIS | keep behind `arch::firmware`; aarch64 can stub measured-boot initially |

**Stays fully shared (no per-arch copy):** scheduler policy + EDF/SCHED_BODY, MM allocator
(buddy + slab + heap — *logic*; only the page-table backend is arch), IPC, VFS, AthFS, all
syscall *handlers*, capability/AthGuard, the entire `components/*` Rae* userspace stack, net
above the NIC, crypto, the procfs surface.

### Failure modes & security model

- **Hot-path regression:** the `arch::` boundary must be `#[inline]`-friendly free functions /
  monomorphized generics — *no `dyn` on context-switch/IRQ/timer paths.* Acceptance includes a
  boot-time-no-regression gate (the `[BOOT-BENCH]` guard, CLAUDE.md §12).
- **Silent arch divergence:** each arch's `mod.rs` must `#[cfg]`-compile *only* on its target;
  CI builds all three targets so a missing seam fn is a **link/compile error**, never a runtime
  surprise.
- **Capability model is arch-invariant:** every privileged op still routes through
  `crate::capability` (CLAUDE.md §4.3) — `arch::` is *mechanism*, never *policy*. No arch
  backend may bypass a `Cap` check. The architecture-gate must learn to reject an `arch/`
  backend that calls a privileged HW op without the shared capability check above it.
- **Trap-frame trust:** the syscall/IRQ entry asm is the one place register state crosses the
  user→kernel boundary; each arch's entry stub MUST zero caller-controlled scratch before
  handing to shared dispatch (x86_64 already does this in `thread_entry_user`).

### 2. Bring-up ORDER + acceptance per arch

Charter order: **x86_64 (done) → aarch64 → i686.** (aarch64 before i686 because aarch64 is the
strategically important target — real ARM hardware, the "not locked to one ISA" proof — and
i686 mostly reuses x86_64's seam shapes, so doing aarch64 second *forces the boundary to be
genuinely arch-neutral* rather than "x86 with 32-bit tweaks.")

| Arch | target triple | QEMU machine | boot path | per-arch acceptance (exact log lines) |
|---|---|---|---|---|
| **x86_64** | `x86_64-unknown-none` (in use) | `qemu-system-x86_64` (current xtask) | `bootloader_api` BIOS+UEFI | `[ OS ] System successfully booted.` + `boot health: 6/6 critical PASS -> HEALTHY` (current baseline; must be **unchanged** after the refactor) |
| **aarch64** | `aarch64-unknown-none-softfloat` | `qemu-system-aarch64 -M virt -cpu cortex-a72 -smp 4` | UEFI (edk2 AAVMF) **or** `-kernel` AArch64 Linux boot protocol; PL011 UART @ `0x0900_0000`, GICv2, ARM generic timer, PSCI for SMP | `[arch] aarch64 cortex-a72 EL1` · `[mmu] TTBR1_EL1 kernel root active` · `[gic] GICv2 dist+cpu online` · `[timer] CNTP_TVAL armed N Hz` · `[smp] PSCI CPU_ON: 4/4 cores online` · then the **same** `[ OS ] System successfully booted.` |
| **i686** | `i686-unknown-none` (custom JSON if absent) | `qemu-system-i386 -smp 4` | `bootloader` BIOS (verify 32-bit support) or 32-bit stub | `[arch] i686 PAE paging` · `[mmu] PAE 3-level CR3 active` · `[irq] IDT + APIC` · `[smp] SIPI: 4/4 cores online` · then `[ OS ] System successfully booted.` |

**Per-arch smoketest (the falsifiable proof beyond the marker):** each arch runs the SAME
shared boot smoketest suite (R10), which now includes an `arch::run_boot_smoketest()` that
asserts: MMU round-trips a known phys↔virt mapping, a self-IPI is delivered and acked, the
monotonic clock advances across a busy-wait, and a context switch into a kernel thread and back
preserves callee-saved regs. Each prints `[arch-smoke] <name> -> PASS|FAIL`. **A test that
can't print FAIL is a false green** (CLAUDE.md §16).

Acceptance per arch = `cargo run -p xtask -- run --target <triple> --ci` exits 0, log shows the
arch lines above + `System successfully booted.` + `[arch-smoke] ... -> PASS`, no `[PANIC]`.
"Install on each arch" (goal criterion) is a **later phase** gated on boot+run first; the
installer is arch-neutral above `arch::mmu`/block I/O, so it follows for free once an arch boots
to userspace with storage.

### 3. The FIRST concrete implementable slice (the safe step)

**Slice 0 — "Introduce `arch::` with zero behavior change."** No aarch64/i686 code. Goal: move
the existing x86_64 seam code behind `kernel/src/arch/x86_64/` and re-export it through
`arch::`, so x86_64 boots *byte-for-byte identically* and a future arch has a slot to fill.

Two sub-slices (commit boundary between them):

- **Slice 0a (interface-only, `athena-architect`):** create `kernel/src/arch/mod.rs` with the
  `#[cfg]` re-export skeleton and the **written seam contract** (the function-signature list
  above, as rustdoc the implementer must satisfy). Add the `arch::run_boot_smoketest()` slot.
  No moves yet — `arch::mod.rs` just `pub use`s the existing modules (`pub use crate::apic`,
  etc.) so nothing breaks. This is the `[interface]`-tagged commit.
- **Slice 0b (mechanical move, `athena-kernel`):** physically relocate
  `context.rs`/`gdt.rs`/`apic.rs`/`smp.rs`/the paging half of `memory.rs`/etc. into
  `arch/x86_64/` and update `arch::mod.rs` to re-export from the new paths. **No logic edits.**
  Callers that did `crate::apic::x` keep working via `pub use`. The hardest part is
  **de-x86_64-crate-typing the MM public surface** — but for Slice 0 we DEFER that: keep
  `OffsetPageTable`/`VirtAddr` for now, just behind `arch::mmu`. (Replacing those newtypes with
  arch-neutral ones is **Slice 1**, still x86_64-only, still QEMU-verifiable, and de-risks
  aarch64 before any aarch64 code exists.)

**What proves Slice 0:** identical boot. After 0a+0b:
```
cargo run -p xtask --release -- build --release      # exit 0
cargo run -p xtask --release -- run --release --ci   # exit 0
```
log MUST still show `System successfully booted.` AND `boot health: 6/6 critical PASS ->
HEALTHY` AND no `[PANIC]` AND `[BOOT-BENCH]` not regressed. Because it's an SMP path,
**≥5 boots at `ATHENA_SMP=1` and `=2`** (CLAUDE.md §17). This is pure-refactor risk only —
the safest possible first step, and it's the precondition for *any* second arch.

**Slice 1 (still x86_64-only):** introduce arch-neutral `arch::mmu::{PhysAddr, VirtAddr, Frame,
Page, AddressSpace}` newtypes; convert `memory.rs` + the ~31 `use x86_64` callers to them, with
x86_64 impls wrapping the `x86_64` crate underneath. Proof: same boot baseline. This is the
single highest-fan-out de-risk for aarch64 (kills the 385 `x86_64::` references' leak into
shared code).

**Only then Slice 2+:** stub `arch/aarch64/`, get it to compile for `aarch64-unknown-none-soft
float`, then bring up boot→UART→MMU→GIC→timer→PSCI→smoketest in QEMU `virt`.

## Interface needs (NEEDS-INTERFACE)

For `athena-architect` (these are the contract, owned as an internal `arch::` seam doc, not
`ath_abi` — this is *intra-kernel*, no syscall/ABI bump):

- The seam function-signature set in the table above (boot/cpu/mmu/irq/time/smp/context/
  syscall/percpu/io/firmware), expressed as the required `pub fn`/associated-type surface of
  `kernel/src/arch/<name>/mod.rs`.
- A shared `BootContext` struct (memmap, framebuffer, firmware-table ptr (RSDP **or** DTB),
  cmdline) that `arch::boot` produces and shared `kernel_main` consumes — replacing the direct
  `bootloader_api::BootInfo` dependency in `kernel_main`'s signature.
- `arch::run_boot_smoketest()` R10 slot.
- **No `ath_abi`/`ath_driver_api` change** (the user-facing ABI is arch-neutral by design; only
  syscall *entry mechanism* differs, and that's internal). Confirm: `ABI_VERSION` unchanged.

## File-by-file plan

- `kernel/src/arch/mod.rs`: NEW — `#[cfg]` re-export + seam contract rustdoc + smoketest slot.
- `kernel/src/arch/x86_64/{boot,cpu,mmu,irq,time,smp,context,syscall_entry,io,percpu,firmware}.rs`:
  NEW homes for moved x86_64 code (Slice 0b). Mechanical move, no logic change.
- `kernel/src/main.rs`: `kernel_main` keeps `bootloader_api` entry but immediately builds a
  `BootContext` and calls a shared inner `kernel_main(ctx)` (Slice 1 makes the inner one
  arch-neutral).
- `kernel/src/memory.rs`: Slice 1 — swap `x86_64` newtypes for `arch::mmu` newtypes.
- `xtask/src/main.rs`: add `--target <triple>` plumbing (default `x86_64-unknown-none`),
  per-arch QEMU selection (`qemu-system-x86_64` / `-aarch64` / `-i386`) + per-arch machine
  args (the `-M virt -cpu cortex-a72 -smp 4` set for aarch64). Today every target/qemu string
  is hardcoded x86_64 (`xtask/src/main.rs:281,322,1324,...`).
- `rust-toolchain.toml`: add `aarch64-unknown-none-softfloat` (and i686 target) to `targets`.
- `.cargo/config.toml`: per-target `rustflags` blocks (the x86_64 frame-pointer/curve25519 flags
  are currently under `[target.x86_64-unknown-none]` only).
- `docs/components/arch.md`: NEW — the `arch::` seam reference (companion to this spec).

## Acceptance criteria (the exact proof)

- **Slice 0/1 (x86_64 unchanged):** boot log MUST still show `[ OS ] System successfully
  booted.` AND `boot health: 6/6 critical PASS -> HEALTHY`; `[BOOT-BENCH]` not regressed; no
  `[PANIC]`; ≥5 boots at `ATHENA_SMP=1` and `=2`. Docstring of `arch/mod.rs` MUST quote the
  Manifesto "locked behind ... Apple silicon" line above.
- **aarch64:** boot log MUST show `[arch] aarch64 ... EL1 -> PASS`, `[mmu] TTBR1_EL1 ...`,
  `[gic] GICv2 ... online`, `[timer] CNTP ...`, `[smp] PSCI CPU_ON: N/N`, `[arch-smoke]
  aarch64 -> PASS`, then `System successfully booted.` under `qemu-system-aarch64 -M virt`.
- **i686:** boot log MUST show `[arch] i686 PAE ...`, `[mmu] PAE 3-level ...`, `[smp] SIPI:
  N/N`, `[arch-smoke] i686 -> PASS`, then `System successfully booted.` under
  `qemu-system-i386`.
- `/proc/athena/arch` MUST report: active arch, CPU count online, page-table levels, IRQ
  controller name, timer name, syscall-entry mechanism.
- Each `arch::run_boot_smoketest()` MUST be able to print FAIL (MMU map mismatch, IPI not acked,
  clock not advancing, or context-switch reg corruption).

## Handoff

- **athena-architect:** owns the `arch::` seam contract (Slice 0a, `[interface]`-style internal
  contract; confirm `ABI_VERSION` unchanged). Produces the seam doc + `BootContext` shape.
- **athena-kernel:** Slice 0b (mechanical move) + Slice 1 (de-x86_64-type the MM) + xtask
  multi-target plumbing.
- **future athena-arch (or athena-kernel):** per-arch impls — aarch64 first (boot/UART/MMU/GIC/
  timer/PSCI/context/svc-entry), then i686.
- **Unblocks:** owner goal criterion #3 (multi-arch boot/run/install). No MasterChecklist line
  exists yet — **recommend the lead add a Phase for "Multi-architecture support"** with the
  per-arch acceptance lines above as its sub-items.
- **Sequencing:** Slice 0a (interface) commits FIRST → Slice 0b → Slice 1 (all x86_64-only,
  each independently QEMU-verifiable and revertible) → aarch64 → i686. Do **not** write any
  aarch64 code until Slice 1 lands (arch-neutral MM types) — otherwise the boundary ossifies
  around x86_64 assumptions and aarch64 forces a second refactor.

## Risks & scope reality

- **This is large.** aarch64 from a from-scratch x86_64 kernel is a multi-month workstream: a
  new boot path (no `bootloader_api`), a new IRQ controller (GIC), DeviceTree replacing ACPI, a
  new context-switch asm, PSCI SMP, the generic timer, and the MM two-TTBR change. Redox spent
  a Summer-of-Code on the aarch64 port *with* their abstraction already mature.
- **The `bootloader_api` dependency is a hard wall for aarch64** — budget a separate spec for
  the aarch64 boot/entry + firmware-table (DTB) story before coding it.
- **Firmware discovery (ACPI vs DTB) is a second large seam** the size-estimate must include —
  the kernel currently assumes ACPI/MADT/SMBIOS everywhere device topology is read.
- **Driver reality:** the Rae* userspace and net stack are arch-neutral, but **every in-kernel
  hardware driver** (nvme/ahci/xhci/rtl8125/HDA) assumes MMIO+x86 IRQ routing; they'll need
  re-validation per arch (most are MMIO and should port, but IRQ delivery differs).
- **Don't spec mass for mass's sake:** Slice 0 adds *zero capability* by itself — its value is
  strictly "unblocks a real feature (a second arch)." Justified because criterion #3 is a hard
  goal. But each x86_64-only slice (0,1) MUST keep boot green and fast or it's just churn.
- **Hot-path tax:** the one technical trap is letting the boundary become `dyn`-dispatched.
  Keep it monomorphized; the `[BOOT-BENCH]` gate is the live tripwire.
- **Confidence the x86_64 refactor is safe:** high (mechanical, fully QEMU-gated). **Confidence
  on aarch64 effort estimate:** medium — the boot/firmware story has unknowns that warrant a
  dedicated follow-up spec before commitment.

---
Sources:
- [Redox kernel src/arch (GitLab)](https://gitlab.redox-os.org/redox-os/kernel)
- [Redox ARM-AARCH64-PORT-OUTLINE.md](https://github.com/redox-os/kernel/blob/master/ARM-AARCH64-PORT-OUTLINE.md)
- [Redox aarch64 PORT-HOWTO](https://gitlab.redox-os.org/redox-os/kernel/blob/master/src/arch/aarch64/doc/PORT-HOWTO.md)
- [RSoC: Porting Redox to AArch64](https://www.redox-os.org/news/rsoc-arm64-0x01/)
- [rust-osdev/bootloader (x86-only)](https://github.com/rust-osdev/bootloader)
- [bootloader_api crate](https://crates.io/crates/bootloader)
- [QEMU AArch64 Virt Bare Bones (OSDev)](https://wiki.osdev.org/QEMU_AArch64_Virt_Bare_Bones)
- [QEMU virt Armv8-A / PSCI / GIC (Trusted Firmware-A)](https://trustedfirmware-a.readthedocs.io/en/latest/plat/qemu.html)
