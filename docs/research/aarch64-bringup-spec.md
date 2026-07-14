# Spec: aarch64 (ARM64) bring-up — boot + smoketests green in QEMU `virt`

Status: RESEARCH / executable plan. **SPEC ONLY — this document touches no kernel code, no
`Cargo.toml`, no `xtask`.** It is the follow-up the multi-arch ADR explicitly deferred
(ADR 0007 §5: *"aarch64 boot/DTB-firmware needs its own follow-up spec before coding it"*).
It extends — does not re-plan — `docs/research/multi-arch-abstraction.md` and `arch/mod.rs`
Slice 0 (already landed, commit 5079a90).

## Concept promise served

> "macOS got locked behind a walled garden of Apple silicon. … AthenaOS is the third path —
> a from-scratch, embodiment-first, native-feeling OS that treats power users like adults"
> (LEGACY_GAMING_CONCEPT.md §The OS Manifesto)

and the north-star clause ADR 0007 added to §Architecture ("Architecture Reach"), already
quoted verbatim in `kernel/src/arch/mod.rs`:

> "AthenaOS refuses ISA lock-in: the kernel sits on a clean `arch::` abstraction layer … so the
> same OS boots x86_64, aarch64 (ARM 64-bit), and i686 (32-bit x86) — each proven independently."

aarch64 is the *load-bearing* proof of that clause: it's the ISA macOS is welded to, and the one
that forces the `arch::` boundary to be genuinely neutral rather than "x86 with tweaks."

## What "aarch64 boots in QEMU" means — the `[x]` definition (verifier-checkable)

Goal criterion #3 says each arch is "proven independently in QEMU." For aarch64 the `[x]` bar is:

```
qemu-system-aarch64 -M virt -cpu cortex-a72 -smp 4 -m 512 \
  -nographic -serial mon:stdio \
  -kernel target/aarch64-unknown-none-softfloat/release/kernel \
  ... (xtask --target aarch64-... --ci adds the disk/exit-on-marker plumbing)
```
exits 0, and `$env:TEMP\raeen-serial.log` contains, with NO `[PANIC]`:
```
[arch] aarch64 cortex-a72 (ptr=64, page=4096) HAL online — EL1
[mmu] TTBR1_EL1 kernel root active (4KB granule, 48-bit VA)
[gic] GICv2 dist@0x08000000 cpu@0x08010000 online
[timer] generic timer CNTP armed, freq=<N> Hz
[smp] PSCI CPU_ON: 4/4 cores online
[arch-smoke] aarch64 -> PASS
[ OS ] System successfully booted.
```
The final two lines are the falsifiable gate. `[arch-smoke] aarch64 -> PASS` is the shared
R10 `arch::run_boot_smoketest()` (already wired at `main.rs:418`) asserting, on aarch64: an MMU
phys↔virt round-trip, a self-IPI (SGI) delivered+acked, the monotonic clock advancing across a
busy-wait, and a kernel-thread context switch preserving callee-saved registers. Per CLAUDE.md
§16 each assertion can print `-> FAIL`.

**Honesty:** this is a multi-week epic (new boot path, MMU from scratch, GIC, generic timer,
PSCI SMP, new context-switch asm, DeviceTree replacing ACPI). Redox spent a Summer-of-Code on
its aarch64 port *with its arch abstraction already mature* ([RSoC](https://www.redox-os.org/news/rsoc-arm64-0x01/)).
i686 follows aarch64 (charter order) and mostly reuses x86_64 seam shapes, so it is the smaller
follow-on, NOT part of this spec.

## Already in the tree (verify-before-implement)

- **Slice 0 landed** (`kernel/src/arch/mod.rs`, `arch/x86_64/mod.rs`, commit 5079a90): the
  `#[cfg(target_arch=…)]` re-export skeleton + a working x86_64 backend for **three** seams —
  arch identity, CPU control, port I/O — plus the R10 trio (`arch::init()` @ `main.rs:417`,
  `arch::run_boot_smoketest()` @ `main.rs:418`, `/proc/raeen/arch` via `dump_text()`). Status: `[x]`.
- **`compile_error!` guard is live** (`arch/mod.rs:47`): any non-x86_64 `target_arch` fails loudly
  at the boundary. This is the slot the aarch64 backend fills.
- **ADR 0007 accepted** (`docs/decisions/0007-multi-arch-strategy.md`): bring-up order
  x86_64→aarch64→i686; `arch::` is monomorphized free-fns/assoc-types (NO `dyn`); the user/syscall
  ABI stays arch-neutral (NO `rae_abi`/`ABI_VERSION` change); the heavy Slice-0b refactor is its
  own serialized round.
- **The seam taxonomy + per-arch acceptance table already exist** in
  `docs/research/multi-arch-abstraction.md` §"The seam list". **This spec does NOT re-derive it** —
  it drills the aarch64 column into an implementable, QEMU-virt-specific, slice-ordered plan.
- **The x86_64 seam modules are still flat** (`kernel/src/{gdt,interrupts,apic,smp,context,
  memory,msr,...}.rs`), NOT yet under `arch/x86_64/`. So Slice 0b (relocate behind `arch::`
  traits, x86 still the only impl) is **not yet done** and is the precondition for everything here.
- **Boot entry is `bootloader_api` `entry_point!(kernel_main, …)`** (`main.rs:307,318,336`). This
  crate is **x86_64-only** — a hard wall for aarch64. aarch64 needs its own entry (this spec's
  Slice A3). `kernel_main` takes `&'static mut BootInfo` and immediately captures TSC via `rdtsc`
  (`main.rs:344`) — both x86-specific and both must move behind `arch::boot` + `BootContext`.
- **xtask hardcodes x86_64** everywhere (`xtask/src/main.rs:281,323,356,1324`): target triple
  `x86_64-unknown-none`, `qemu-system-x86_64`. No `--target` plumbing yet (Slice A2).

**Delta to build:** the entire aarch64 column of the seam table, plus the xtask multi-target
plumbing, plus the prerequisite x86-only refactor slices (0b, 1) that ADR 0007 already scoped.

## Prior art & OSS verdict (aarch64-specific, current sources)

- **Redox `src/arch/aarch64`** — closest Rust model. Boot = an A64 asm stub that sets up the MMU
  from scratch, parses the DTB (x0), brings up GICv2 + the ARM generic timer, and uses PSCI for
  secondary cores. **Key insight, already captured in the predecessor spec:** x86_64 has one
  `cr3`; aarch64 has **two** table-base registers — `TTBR0_EL1` (low/user half) and `TTBR1_EL1`
  (high/kernel half) — so the paging seam must name *which* root. Verdict: **📖 study/isolate** —
  Redox is MIT but `redox_reference/` is harvest-only (CLAUDE.md §10.13). Harvest the layering +
  two-TTBR pattern, not code. Sources:
  [PORT-HOWTO](https://gitlab.redox-os.org/redox-os/kernel/blob/master/src/arch/aarch64/doc/PORT-HOWTO.md),
  [AARCH64 outline](https://github.com/redox-os/kernel/blob/master/ARM-AARCH64-PORT-OUTLINE.md).
- **`fdt` crate (MIT/Apache)** — flattened-DeviceTree parser; replaces the x86 ACPI/MADT/SMBIOS
  path for CPU/IRQ/device discovery on aarch64. The QEMU virt machine *builds the DTB at runtime*
  and hands its base in x0, so this is the firmware-discovery backend. Verdict: **➕ vendorable**
  when aarch64 reaches device discovery (Slice A8). Until then a *static* QEMU-virt memmap (table
  below) is enough to boot — DTB parse is deferred so the first boot doesn't block on a parser.
- **`uefi` crate (MIT/Apache)** — works on aarch64; an alternative entry via edk2 AAVMF firmware.
  Verdict: **➕ vendorable**, but the spec recommends the **`-kernel` direct-load AArch64 Linux
  boot protocol** first (simpler, no firmware blob, deterministic for CI) and treats UEFI/AAVMF as
  a later iron-path option.
- **Trusted Firmware-A QEMU port docs** — authoritative for PSCI conduit + EL on `virt`. Default
  boot is EL1; PSCI is provided by QEMU's in-built implementation, conduit = HVC when EL2/EL3 are
  present, else SMC. Source: [TF-A QEMU plat](https://trustedfirmware-a.readthedocs.io/en/latest/plat/qemu.html),
  [QEMU virt docs](https://www.qemu.org/docs/master/system/arm/virt.html).
- **`bootloader` / `bootloader_api`** — **in-use for x86, NOT usable for aarch64** (x86_64-only
  project). aarch64 must NOT depend on it. Verified against the crate.

No architecture is transplanted (Concept §R7 / CLAUDE.md §4.2): we copy the universal per-arch
module-tree idea + Redox's two-TTBR insight only.

## The QEMU `virt` machine memory map (authoritative — the de-risk core)

From QEMU's own `hw/arm/virt.c` `base_memmap[]`
([source](https://github.com/qemu/qemu/blob/master/hw/arm/virt.c)). These are the MMIO bases the
aarch64 backend hardcodes for the first boot (DTB parse later refines them):

| Region | Base | Size | AthenaOS use |
|---|---|---|---|
| VIRT_FLASH | `0x0000_0000` | `0x0800_0000` | boot ROM (`-kernel` loads here / pflash) |
| **VIRT_GIC_DIST** | `0x0800_0000` | `0x0001_0000` | GIC **distributor** (GICD_*) |
| **VIRT_GIC_CPU** | `0x0801_0000` | `0x0001_0000` | GICv2 **CPU interface** (GICC_*) |
| VIRT_GIC_REDIST | `0x080A_0000` | `0x00F6_0000` | GICv3 redistributor (only if GICv3 selected) |
| **VIRT_UART0** | `0x0900_0000` | `0x0000_1000` | **PL011 UART** — first-serial-byte target |
| VIRT_RTC | `0x0901_0000` | `0x0000_1000` | PL031 RTC (wall-clock; later) |
| VIRT_MMIO | `0x0A00_0000` | `0x200`×32 | virtio-mmio transports (disk/net later) |
| VIRT_PCIE_MMIO | `0x1000_0000` | `0x2EFF_0000` | PCIe memory window (later) |
| VIRT_PCIE_ECAM | `0x3F00_0000` | `0x0100_0000` | PCIe config space (later) |
| **VIRT_MEM** | `0x4000_0000` | `-m` size | **main RAM** — kernel loads at `0x4008_0000` per the AArch64 boot protocol's 2 MiB text offset |

**Default GIC on `-M virt` is GICv2** (CPU interface MMIO at `0x0801_0000`); GICv3 is selectable
(`-M virt,gic-version=3`) and uses the redistributor region + system-register CPU interface. **The
spec targets GICv2 first** (MMIO CPU interface is simpler to bring up than the GICv3 `ICC_*`
sysreg dance); GICv3 is a follow-on once GICv2 is green.

**Boot exception level on `-M virt` is EL1** by default (EL2/EL3 only with
`-M virt,virtualization=on`/`secure=on`). So the backend assumes **entry at EL1** and does NOT
need the EL2→EL1 drop for the QEMU CI path — but the entry stub MUST read `CurrentEL` and, if it
*does* find itself at EL2 (real hardware / `virtualization=on`), perform the EL2→EL1 transition
(set `HCR_EL2.RW=1`, `SPSR_EL2`, `ELR_EL2`, `eret`). PSCI conduit follows: **HVC** if EL2/EL3
present, else **SMC** — the backend probes via the DTB `psci` node's `method` property (deferred:
hardcode SMC for the EL1-only QEMU CI path, the documented default).

**AArch64 Linux boot protocol contract** (what QEMU `-kernel` guarantees at entry):
`x0` = physical address of the DTB; `x1=x2=x3=0`; MMU **off**; D-cache off; I-cache may be on;
the CPU is the primary, secondaries are held in PSCI (released via `CPU_ON`). The kernel image
header's first instruction is the entry point. **This is the entry the backend must satisfy** —
it is NOT a `bootloader_api` entry.

## Design — the arch:: surface aarch64 fills

aarch64 fills the *exact same seam contract* x86_64 already exposes (per ADR 0007 — the contract
is the set of required `pub fn`/`const`/assoc-types in `arch/<name>/mod.rs`, link-enforced). The
aarch64-specific realizations:

- **Identity consts:** `NAME="aarch64"`, `POINTER_WIDTH=64`, `PAGE_SIZE=4096`,
  `IS_LITTLE_ENDIAN=true`, `INTERRUPT_CONTROLLER="GICv2"`, `TIMER_SOURCE="ARM generic timer"`.
- **CPU control:** `cpu_relax()` → `yield` (`WFE`-free hint); `halt()` → `wfi`;
  `interrupts_enabled()` → read `DAIF.I`; `disable/enable_interrupts()` → `msr daifset/daifclr,#2`;
  `without_interrupts()` → save+restore `DAIF`. (Same contract as x86's IF save/restore, so the
  existing `arch::run_boot_smoketest()` IF-save/restore assertion works unchanged.)
- **Port I/O:** aarch64 has **no port space.** `port::{inb/outb/…}` must be **`cfg`'d out** on
  aarch64 (callers that use ports are x86-only firmware paths that don't compile on aarch64) —
  or lowered to a `compile_error!`/`panic!` so a stray caller is caught. MMIO is the only path.
- **MMU (the biggest seam):** 4-level, 4 KiB granule, 48-bit VA. Two roots: `TTBR0_EL1` (low VA,
  user) + `TTBR1_EL1` (high VA, kernel — the `0xFFFF_…` half selected by `TCR_EL1.T1SZ`). The
  identity bring-up map is built in the entry stub before `sctlr_el1.M=1`. Memory attributes go
  through `MAIR_EL1` (index 0 = Normal WB, index 1 = Device-nGnRnE for MMIO). `TCR_EL1` sets
  T0SZ/T1SZ=16 (48-bit), 4 KiB granule (TG0/TG1), inner/outer WB cacheable walks. The `arch::mmu`
  seam carries "which root" (kernel vs user) per the Redox two-TTBR insight.
- **Exceptions:** `VBAR_EL1` points at a **16-entry, 0x80-aligned vector table** (4 groups ×
  {Sync, IRQ, FIQ, SError}: current-EL-SP0, current-EL-SPx, lower-EL-AArch64, lower-EL-AArch32).
  The Sync vector decodes `ESR_EL1.EC` to route page-fault (data/instruction abort) vs `SVC`
  (syscall, `EC=0x15`) vs undefined. The shared page-fault *policy* stays in the kernel; the
  aarch64 vector just builds the trap frame + reads `FAR_EL1` for the faulting address.
- **Timer:** the **ARM generic timer** is the LAPIC-timer equivalent. Frequency from `CNTFRQ_EL0`;
  arm a one-shot by writing `CNTP_TVAL_EL0` (downcount) and `CNTP_CTL_EL0.ENABLE=1, IMASK=0`. The
  timer fires **PPI 30** (physical) / 27 (virtual) into the GIC. `now_ns` reads `CNTPCT_EL0`.
- **GIC (GICv2):** distributor (`GICD_*` @ `0x0800_0000`): `GICD_CTLR` enable, `GICD_ISENABLER`
  to unmask SGIs/PPIs/SPIs, `GICD_IPRIORITYR`. CPU interface (`GICC_*` @ `0x0801_0000`):
  `GICC_CTLR` enable, `GICC_PMR=0xFF` (allow all priorities), `GICC_IAR`/`GICC_EOIR` for
  ack/end-of-interrupt — the **EOI** is the GIC analogue of the x86 APIC EOI. Self-IPI for the
  smoketest = `GICD_SGIR` (Software Generated Interrupt).
- **SMP:** **PSCI `CPU_ON`** (function ID `0xC400_0003`) via `HVC`/`SMC`, passing the secondary's
  MPIDR target + the physical entry point + a context-id. Replaces INIT-SIPI-SIPI entirely; no
  real-mode trampoline (aarch64 has no real mode). `this_cpu_id()` reads `MPIDR_EL1.Aff0`.
- **Context switch:** save/restore `x19–x30` (callee-saved) + `sp` + (later) `q8–q15`
  FP/SIMD; the address-space swap is a `TTBR0_EL1` write + `tlbi`/`dsb`/`isb`. New seam asm,
  same shape as x86's `switch_context`.
- **Syscall entry:** userspace `svc #0` traps to the Sync vector; `ESR_EL1.EC==0x15` routes to the
  arch entry stub, which marshals `x0–x7`+`x8`(nr) and calls the **shared** `syscall::dispatch` —
  the 3296-line dispatch is arch-neutral and does NOT move.
- **Per-CPU base:** `TPIDR_EL1` (kernel) replaces `GS_BASE`.

### Security model (unchanged, arch-invariant)
- Every privileged op still routes through `crate::capability` — `arch::` is **mechanism, never
  policy** (CLAUDE.md §4.3). No aarch64 backend bypasses a `Cap` check; the architecture-gate must
  reject an `arch/aarch64` HW op without the shared capability check above it.
- The `svc`/IRQ entry stub is the one user→kernel register-state crossing: it MUST zero
  caller-controlled scratch before handing to shared dispatch (mirrors x86 `thread_entry_user`).
- **CLAUDE.md §10 keystones port directly:** the masked-context/syscall-stack rule (a blocked task
  must set the per-CPU syscall stack — on aarch64 the equivalent is the `SP_EL1`/exception stack
  pointer, same hazard class), and the contiguous-frame rule (`allocate_contiguous_frames(order)`
  for multi-page phys buffers — the aarch64 MMU page-table builder allocates table pages and MUST
  use it, not a `allocate_frame()` loop). These are called out per-slice below.

## Slices — in dependency order, each separately landable + verifiable

Every slice keeps x86_64 booting green (the live `[boot] WARN`/`[BOOT-BENCH]` gates apply) until
the aarch64 column exists, then proves aarch64 incrementally. Slices 0b and 1 are the ADR-0007
prerequisites (x86-only refactor) restated for completeness; A2–A9 are the new aarch64 work.

### Slice 0b — relocate seams behind `arch::` (x86 still the only impl) — **PREREQUISITE**
- **Owner:** raeen-kernel. **Files:** move `{gdt,interrupts,apic,smp,context,msr,cpu_features,
  hpet,timers}.rs` + the paging half of `memory.rs` into `kernel/src/arch/x86_64/`; widen
  `arch/x86_64/mod.rs` + `arch/mod.rs` to the full seam contract (mmu/irq/time/smp/context/
  syscall_entry/percpu/firmware), re-export so `crate::apic::x` callers keep working via `pub use`.
  **No logic edits.** Defer the MM newtype change to Slice 1.
- **arch:: surface added:** the full seam fn-signature set (the `NEEDS-INTERFACE` raeen-architect
  owns as an internal contract doc — NOT `rae_abi`, NO `ABI_VERSION` bump).
- **Proof line:** UNCHANGED x86 boot — `[ OS ] System successfully booted.` + `boot health: 6/6
  critical PASS -> HEALTHY`, no `[PANIC]`, `[BOOT-BENCH]` not regressed. **≥5 boots at
  `RAEEN_SMP=1` and `=2`** (CLAUDE.md §17 — it touches the SMP path).
- **Verifiable immediately** as "still 7/7 on x86_64." Pure-refactor risk only.

### Slice 1 — arch-neutral MM newtypes (`PhysAddr/VirtAddr/Frame/Page/AddressSpace`) — **PREREQUISITE**
- **Owner:** raeen-kernel. **Files:** `arch/x86_64/mmu.rs` (x86 impls wrapping the `x86_64`
  crate), `memory.rs` + the ~31 `use x86_64` callers swap to `arch::mmu::*`. Kills the 385
  `x86_64::` leaks into shared code — the single highest-fan-out aarch64 de-risk.
- **Proof line:** same x86 baseline (`System successfully booted.` + `6/6 critical PASS`). Host-KAT
  the newtype arithmetic (align-up/down, page-index, canonical-form checks) on the dev box first.
- **Gate:** ADR 0007 — **no aarch64 code until Slice 1 lands**, else the boundary ossifies around
  x86 assumptions.

### Slice A2 — aarch64 target triple + xtask build path (compiles, doesn't boot)
- **Owner:** raeen-kernel. **Files:** `rust-toolchain.toml` (+`aarch64-unknown-none-softfloat`),
  `.cargo/config.toml` (per-target rustflags block), `xtask/src/main.rs` (`--target <triple>`
  plumbing, default `x86_64-unknown-none`), `arch/aarch64/mod.rs` (NEW — identity consts +
  `unimplemented!`-free *compiling* stubs that `panic!("aarch64 <seam> not yet up")` so it links).
  Remove `arch/aarch64` from the `compile_error!` exclusion in `arch/mod.rs`.
- **arch:: surface added:** the aarch64 backend module shell satisfying the link contract.
- **Proof line:** `cargo build -p kernel --target aarch64-unknown-none-softfloat` exits 0
  (an **aarch64 ELF is produced**). No boot yet. CI gains an aarch64 *build* job.
- **Note:** `softfloat` triple chosen because the kernel is `#![no_std]` + soft-float (mirrors
  x86's soft-float, CLAUDE.md memory `kernel-soft-float-no-fpu-save`); FP/SIMD context save is a
  later refinement, not a boot blocker.

### Slice A3 — boot entry + EL1 + UART (first serial byte on aarch64)
- **Owner:** raeen-arch (core) + raeen-architect (the `BootContext` shape, if not done in 0b).
  **Files:** `arch/aarch64/boot.rs` (the A64 entry stub: image header, read `CurrentEL`, drop
  EL2→EL1 *if* needed, set up `SP_EL1`, stash `x0`=DTB, clear BSS, call shared init),
  `arch/aarch64/io.rs` (PL011 UART @ `0x0900_0000`: write `UARTDR`, poll `UARTFR.TXFF`).
- **arch:: surface:** `arch::boot::_start` normalizing to the shared `BootContext` (memmap from
  the static QEMU-virt table for now; DTB ptr stashed for Slice A8); the `serial`/early-print path
  routes to `arch::io` PL011 on aarch64.
- **Proof line:** `[arch] aarch64 cortex-a72 (ptr=64, page=4096) HAL online — EL1` appears on the
  QEMU serial. **First aarch64 serial byte.** (Boot will then `panic!`/hang at the next unbuilt
  seam — that's expected; the line printing IS the proof.)

### Slice A4 — MMU + exception vectors
- **Owner:** raeen-arch. **Files:** `arch/aarch64/mmu.rs` (build identity + higher-half tables,
  program `MAIR_EL1`/`TCR_EL1`/`TTBR0_EL1`/`TTBR1_EL1`, `sctlr_el1.M=1`, `isb`),
  `arch/aarch64/irq.rs` (the 16-entry `VBAR_EL1` table + Sync handler decoding `ESR_EL1.EC`,
  reading `FAR_EL1`).
- **arch:: surface:** `arch::mmu::{AddressSpace, map, unmap, translate, switch_to, kernel_root}`
  realized for aarch64; `arch::irq` trap-frame builder.
- **Keystone:** the table-page allocator MUST use `allocate_contiguous_frames(order)` for any
  multi-page table region (CLAUDE.md §10.7).
- **Proof line:** `[mmu] TTBR1_EL1 kernel root active (4KB granule, 48-bit VA)` + a deliberate
  test mapping that the `arch-smoke` MMU round-trip later asserts. A faulting access into an
  unmapped page now lands in the aarch64 Sync vector (not a silent reset) — provable by a
  FAIL-able smoketest that maps, reads back, and checks.

### Slice A5 — generic timer + GIC (interrupts live)
- **Owner:** raeen-arch. **Files:** `arch/aarch64/time.rs` (CNTFRQ/CNTPCT/CNTP_TVAL/CNTP_CTL),
  `arch/aarch64/irq.rs` (GICv2 dist+cpu init: `GICD_CTLR`, `GICC_CTLR`, `GICC_PMR`, `ISENABLER`;
  `GICC_IAR`/`GICC_EOIR` ack/eoi; `GICD_SGIR` self-IPI).
- **arch:: surface:** `arch::time::{now_ns, set_oneshot, calibrate, tick_hz}` +
  `arch::irq::{register_handler, eoi, enable, disable, send_ipi}`.
- **Proof lines:** `[gic] GICv2 dist@0x08000000 cpu@0x08010000 online` and
  `[timer] generic timer CNTP armed, freq=<N> Hz`. The timer PPI firing + being EOI'd is what
  drives the scheduler tick; a self-SGI delivered+acked is the `arch-smoke` IPI assertion.

### Slice A6 — PSCI SMP (secondary cores)
- **Owner:** raeen-arch. **Files:** `arch/aarch64/smp.rs` (PSCI `CPU_ON` via HVC/SMC; secondary
  entry stub sets up its own `SP_EL1`/`TTBR`/`VBAR`/GIC CPU-iface; `this_cpu_id` from `MPIDR_EL1`).
- **arch:: surface:** `arch::smp::{cpu_count, start_ap(id, entry), this_cpu_id}`.
- **Keystone:** each secondary's per-CPU syscall/exception stack must be set before it can block
  (CLAUDE.md §10.6 — the worst-bug-in-the-tree class; the aarch64 analogue is `SP_EL1`/the
  exception stack per core).
- **Proof line:** `[smp] PSCI CPU_ON: 4/4 cores online`. Run ≥5 boots (SMP path; CLAUDE.md §17).

### Slice A7 — boot marker + `[arch-smoke] aarch64 -> PASS`
- **Owner:** raeen-arch. **Files:** wire the aarch64 backend into `arch::run_boot_smoketest()` so
  the four shared assertions (MMU round-trip, self-IPI ack, clock advance, context-switch reg
  preservation) execute on aarch64; ensure the shared `kernel_main` tiers that are arch-neutral
  reach `[ OS ] System successfully booted.` under QEMU virt (tiers that need ACPI/x86 firmware
  are `cfg`-gated or DTB-backed — see A8).
- **Proof lines (the `[x]` gate):**
  ```
  [arch-smoke] aarch64 -> PASS
  [ OS ] System successfully booted.
  ```
  under `qemu-system-aarch64 -M virt -cpu cortex-a72 -smp 4`, no `[PANIC]`, xtask `--ci` exit 0.

### Slice A8 — DeviceTree firmware discovery (replaces ACPI)
- **Owner:** raeen-arch. **Files:** `arch/aarch64/firmware.rs` (vendor `fdt` crate; parse the
  DTB at `x0` → CPU count, GIC base/version, UART base, memory size, PSCI method). Replaces the
  static QEMU-virt memmap from A3 with discovered values; makes the backend work on non-virt
  boards. **This can land after A7** (boot works on the hardcoded virt map first).
- **arch:: surface:** `arch::firmware::discover() -> PlatformTopology` (the ACPI-vs-DTB seam).
- **Proof line:** `[fdt] DTB @ 0x<addr>: <N> CPUs, GICv2, UART@0x09000000, <M> MiB RAM` — and the
  prior slices' hardcoded bases now match discovered values (a FAIL-able cross-check).

### Slice A9 — context switch + svc syscall entry (userspace on aarch64)
- **Owner:** raeen-arch. **Files:** `arch/aarch64/context.rs` (callee-saved x19–x30 + sp + TTBR0
  swap), `arch/aarch64/syscall_entry.rs` (`svc #0` → `ESR_EL1.EC==0x15` → marshal x0–x7,x8 →
  shared `syscall::dispatch`). Per-CPU base via `TPIDR_EL1`.
- **arch:: surface:** `arch::context::{switch, new_kernel_thread, new_user_thread, TaskContext}`,
  `arch::syscall::init_entry()`, `arch::percpu::{set_base, this_cpu_ptr}`.
- **Keystone:** the entry stub zeroes caller-controlled scratch before shared dispatch; the block
  path sets the per-core exception stack (CLAUDE.md §10.6).
- **Proof line:** a userspace task spawns and makes a syscall on aarch64 — the existing daemon
  smoketest markers appear in the aarch64 boot log. (This is past the `[x]` boot gate; it's the
  "run" half of "boot/run/install" and unblocks the aarch64 installer for free.)

## The x86-ism inventory → aarch64 equivalent → owning slice (the de-risk table)

| x86-ism (current boot path) | location | aarch64 equivalent | slice |
|---|---|---|---|
| `rdtsc` boot timestamp | `main.rs:344` | `CNTPCT_EL0` read | A3/A5 |
| `bootloader_api entry_point!` + `BootInfo` | `main.rs:307,318,336` | A64 entry stub, `x0`=DTB, AArch64 Linux boot protocol → shared `BootContext` | A3 |
| 16550 COM1 @ port `0x3F8` | `main.rs:371`, `serial.rs` | PL011 UART MMIO @ `0x0900_0000` | A3 |
| port I/O `in`/`out` (`x86_64::…::port`) | `arch/x86_64/mod.rs` port:: | **none** — MMIO only; `cfg` out on aarch64 | A2/A3 |
| GDT + TSS + IST | `gdt.rs`, `main.rs:396` | no GDT; `SP_EL1`/exception stacks + `VBAR_EL1` | A3/A4 |
| `CR4.OSFXSR` SSE enable | `main.rs:402` | FPEN in `CPACR_EL1` (FP/SIMD enable) | A4 |
| IDT + exception handlers + PIC 8259 | `interrupts.rs`, `main.rs:405` | 16-entry `VBAR_EL1` vector table + `ESR_EL1` decode | A4 |
| `rdmsr`/`wrmsr` + `#GP`-recovered probing | `msr.rs`, `main.rs:411` | `MRS`/`MSR` system-register reads (`ID_AA64*`); no #GP-probe pattern | A4 |
| CPUID feature fingerprint | `cpu_features.rs` | `ID_AA64PFR0/ISAR*` system regs | A4 |
| `Cr3` / `OffsetPageTable` (single root) | `memory.rs` | `TTBR0_EL1`+`TTBR1_EL1` (two roots) + `TCR/MAIR_EL1` | 1/A4 |
| x2APIC/LAPIC + EOI | `apic.rs` | GICv2 dist+cpu, `GICC_EOIR` | A5 |
| TSC calibration / LAPIC timer | `apic.rs`, `hpet.rs`, `timers.rs` | generic timer `CNTFRQ`/`CNTP_TVAL`/`CNTP_CTL` | A5 |
| INIT-SIPI-SIPI + real-mode trampoline | `smp.rs`, `smp/trampoline.asm` | PSCI `CPU_ON` (HVC/SMC); no real mode | A6 |
| `GS_BASE` per-CPU | `apic.rs`/percpu | `TPIDR_EL1` | A6/A9 |
| `syscall`/`sysret` via LSTAR/STAR/SFMASK MSRs | `syscall.rs` | `svc #0` → Sync vector (`EC=0x15`) | A9 |
| ACPI (RSDP/MADT/DSDT) + SMBIOS + EFI | `acpi*.rs`, `smbios.rs`, `efi.rs` | DeviceTree (DTB @ x0), `fdt` crate | A8 |
| `switch_context` asm (rbx/rbp/r12-15 + fxsave64 + cr3) | `context.rs` | x19–x30 + sp + (FP) + TTBR0 swap | A9 |

## What stays shared vs what forks (portability debt call-out)

**Stays fully shared — must NOT fork** (pure `no_std`+`alloc` logic, arch-neutral):
scheduler policy + EDF/SCHED_BODY, the MM allocator *logic* (buddy/slab/heap — only the page-table
*backend* is arch), IPC, VFS, AthFS, **every syscall handler** (the 3296-line `syscall::dispatch`),
capability/AthGuard, the entire `components/*` Rae* userspace stack, AthNet above the NIC, crypto,
the procfs surface. The user/syscall **ABI is arch-neutral** — NO `rae_abi`/`ABI_VERSION` change.

**Must be per-arch (the HAL only):** boot/early-init, CPU feature init, MMU/paging backend,
interrupt controller, timer, SMP bring-up, context switch, syscall entry, per-CPU base,
port-vs-MMIO I/O, firmware discovery (ACPI vs DTB).

**Latent portability debt to fix during 0b/1 (currently x86-coupled, shouldn't be):**
- `main.rs:344` captures TSC via inline `rdtsc` *in `kernel_main` itself* — must move behind
  `arch::boot`/`arch::time::now_ns` before `kernel_main` can be shared.
- `kernel_main`'s signature takes `bootloader_api::BootInfo` — must become `BootContext` (Slice 1).
- The 13 files doing port I/O via `x86_64::…::port` (predecessor-spec count) leak port assumptions
  into otherwise-shared driver glue — audit during 0b so aarch64 `cfg`-out is clean.
- **In-kernel hardware drivers** (nvme/ahci/xhci/rtl8125/HDA) assume MMIO + x86 IRQ routing. Most
  are MMIO and should port, but IRQ delivery differs (GIC SPI vs MSI). These are **past the boot
  gate** — flagged here as the "run with real devices" follow-on, NOT a Slice A1–A9 blocker (QEMU
  virt + virtio-mmio gives a disk/net path without porting the x86 NICs).

## Risks + the cheapest proof path

- **Host-KAT-able before boot (do this first, cheapest real proof — CLAUDE.md §15):**
  - The aarch64 **page-table builder** (descriptor encoding: block/table bits, `MAIR` index,
    AP/AF/SH attribute fields, the level-shift arithmetic for 4 KiB/48-bit) is pure logic behind a
    trait — assert known VA→descriptor encodings on the dev box.
  - The **GIC register math** (which `ISENABLER`/`IPRIORITYR` word + bit for IRQ N, `GICD_SGIR`
    encoding for a target self-IPI) is pure arithmetic — KAT it.
  - The **`ESR_EL1.EC` decode table** (sync-abort vs `svc` vs undefined) — KAT the bit extraction.
  - **DTB parse** (Slice A8) — KAT the `fdt` walk against a `qemu -M virt,dumpdtb=virt.dtb` capture
    (deterministic, host-side, no QEMU boot needed: `qemu-system-aarch64 -M virt,dumpdtb=…`).
  These mirror how the x86 ACPI 0→159-device bug was root-caused entirely off-target.
- **The QEMU-aarch64 verifier path** (the charter's "proven independently in QEMU"):
  ```
  cargo run -p xtask --release -- run --target aarch64-unknown-none-softfloat --ci
  ```
  which (after Slice A2 xtask plumbing) invokes
  `qemu-system-aarch64 -M virt -cpu cortex-a72 -smp 4 -m 512 -nographic -serial mon:stdio
   -kernel <aarch64 kernel> …`, waits for `[ OS ] System successfully booted.`, exits 0/1.
  The verifier asserts the per-slice log lines above + no `[PANIC]`.
- **Risks:** (1) `bootloader_api` is a hard wall — the A3 entry stub is genuinely new code with no
  crate to lean on (budgeted). (2) GICv3-vs-GICv2 — spec'd GICv2 first to avoid the `ICC_*` sysreg
  detour. (3) The EL2→EL1 drop is conditional — QEMU CI default is EL1 (no drop), but the stub must
  handle EL2 for real HW; mis-handling = silent hang, so the stub logs `CurrentEL` before/after.
  (4) Don't let the boundary go `dyn` (hot-path tax) — the `[BOOT-BENCH]` gate is the tripwire.
  (5) Don't spec mass for mass's sake — Slices 0b/1 add zero capability alone; their value is
  strictly "unblock a second arch," justified only because criterion #3 is a hard goal.

## Acceptance criteria (the exact proof)

- **Slices 0b/1 (x86 unchanged):** `[ OS ] System successfully booted.` + `boot health: 6/6
  critical PASS -> HEALTHY`, no `[PANIC]`, `[BOOT-BENCH]` not regressed, ≥5 boots `RAEEN_SMP=1`/`=2`.
- **Slice A2:** `cargo build -p kernel --target aarch64-unknown-none-softfloat` exits 0.
- **Slices A3–A7 (the `[x]` aarch64 boot gate):** under `qemu-system-aarch64 -M virt -cpu
  cortex-a72 -smp 4`, the boot log shows, with no `[PANIC]`:
  ```
  [arch] aarch64 cortex-a72 (ptr=64, page=4096) HAL online — EL1
  [mmu] TTBR1_EL1 kernel root active (4KB granule, 48-bit VA)
  [gic] GICv2 dist@0x08000000 cpu@0x08010000 online
  [timer] generic timer CNTP armed, freq=<N> Hz
  [smp] PSCI CPU_ON: 4/4 cores online
  [arch-smoke] aarch64 -> PASS
  [ OS ] System successfully booted.
  ```
- `/proc/raeen/arch` on aarch64 MUST report: `arch=aarch64`, `interrupt_controller=GICv2`,
  `timer=ARM generic timer`, CPUs online, page-table levels (4), syscall-entry mechanism (`svc`).
- `arch::run_boot_smoketest()` on aarch64 MUST be able to print FAIL (MMU map mismatch, SGI not
  acked, `CNTPCT` not advancing, or context-switch reg corruption).
- `arch/aarch64/mod.rs` docstring MUST quote the §Architecture-Reach clause already in `arch/mod.rs`.

## Handoff — the named first slice

- **Execute FIRST: Slice 0b (relocate seams behind `arch::`, x86 still the only impl).**
  - **Owner:** raeen-kernel (core); **raeen-architect** signs off the widened seam contract as the
    internal `arch::` interface (NOT `rae_abi` — confirm `ABI_VERSION` unchanged).
  - **Why first / why immediately verifiable:** it's a pure, fully-QEMU-gated refactor — the proof
    is "x86_64 STILL 7/7 green." It needs no aarch64 code, no new hardware understanding, and it is
    the precondition for *every* aarch64 slice (the seam contract must exist before a second backend
    can fill it). It de-risks the whole epic at the lowest possible cost.
  - **Boot-log proof line:** `[ OS ] System successfully booted.` + `boot health: 6/6 critical
    PASS -> HEALTHY`, no `[PANIC]`, `[BOOT-BENCH]` not regressed, ≥5 boots at `RAEEN_SMP=1` and `=2`.
- **Then Slice 1** (arch-neutral MM newtypes, raeen-kernel) — same x86 baseline, host-KAT the
  newtype arithmetic. **No aarch64 code until Slice 1 lands** (ADR 0007 gate).
- **Then A2 → A9** (raeen-arch), per the order above. **Recommend the lead add a MasterChecklist
  Phase "Multi-architecture: aarch64"** with A2–A9 as sub-items and the per-slice proof lines as
  acceptance. i686 is a separate follow-on spec after aarch64 is green.

---
Sources:
- [QEMU `hw/arm/virt.c` `base_memmap[]`](https://github.com/qemu/qemu/blob/master/hw/arm/virt.c)
- [QEMU `virt` machine docs (GIC version, EL, virtio)](https://www.qemu.org/docs/master/system/arm/virt.html)
- [Trusted Firmware-A — QEMU platform (PSCI conduit, EL)](https://trustedfirmware-a.readthedocs.io/en/latest/plat/qemu.html)
- [Redox aarch64 PORT-HOWTO](https://gitlab.redox-os.org/redox-os/kernel/blob/master/src/arch/aarch64/doc/PORT-HOWTO.md)
- [Redox ARM-AARCH64-PORT-OUTLINE](https://github.com/redox-os/kernel/blob/master/ARM-AARCH64-PORT-OUTLINE.md)
- [RSoC: Porting Redox to AArch64](https://www.redox-os.org/news/rsoc-arm64-0x01/)
- [coreboot QEMU AArch64 emulator notes](https://doc.coreboot.org/mainboard/emulation/qemu-aarch64.html)
- Predecessor spec: `docs/research/multi-arch-abstraction.md`; ADR: `docs/decisions/0007-multi-arch-strategy.md`
