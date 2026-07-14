# Spec: Architecture Abstraction Layer (multi-arch: x86_64 → aarch64 → i686)

## Concept promise served

> "Windows became bloated chasing enterprise. macOS got locked behind a walled garden of Apple silicon. Linux never figured out gaming or design coherence. RaeenOS is the third path…" (§Thesis)

> "Memory tagging on supported CPUs (ARMv8.5 MTE, Intel/AMD equivalent as it ships)" (§Security Model)

The Concept doc never says "x86_64 only." Its entire pitch is that RaeenOS is the answer to *macOS being locked to Apple silicon* — which is only credible if RaeenOS itself can run on ARM (the Rae Station, gaming handhelds, ARM laptops) as well as x86. The §Security Model explicitly names ARMv8.5 MTE as a first-class hardware feature. Multi-arch reach is therefore latent in the Concept, and this spec makes it buildable without a single regression to the iron-proven x86_64 path.

This spec is **research + boundary design only**. It writes no kernel code; it defines the seam, the module layout, the build wiring, the phased plan, and the exact boot-log proof for each phase. Charter priority #3.

---

## Already in the tree (verify-before-implement)

There is **no `kernel/src/arch/` directory** today (`ls kernel/src/arch` → absent) and **no arch trait**. The kernel is x86_64-only and arch code is scattered across ~25 files at the top level of `kernel/src/`. Build target is hardcoded `x86_64-unknown-none` in three places in `xtask/src/main.rs` (lines 184, 226, 259, 287) and in `rust-toolchain.toml` (`targets = ["x86_64-unknown-none"]`). Status of multi-arch work: **`[ ]` — nothing exists.** MasterChecklist has no aarch64/i686 line (grep: only line 936 "Memory tagging (MTE on ARM future)").

### Inventory — the x86_64-specific surface that must move behind `arch`

Cited with real paths + the load-bearing functions/items. This is the exact set the Phase-0 boundary has to wrap.

| Concern | File | x86_64-specific items (functions / asm / crate use) |
|---|---|---|
| **Entry / boot info** | `kernel/src/main.rs:301-312,330` | `bootloader_api::entry_point!(kernel_main, …)`, `BootloaderConfig`, `BootInfo` (rust-osdev bootloader is x86-only); raw `rdtsc` at :334-345 |
| **GDT / TSS** | `kernel/src/gdt.rs` (entire, 261 lines) | `GlobalDescriptorTable`, `TaskStateSegment`, `Descriptor::kernel_code_segment`, `load_tss`, `CS/SS/DS::set_reg`, `GsBase::write`, `init()`, `init_ap_percpu()`, `set_rsp0()`, `current_cpu_id()` (reads GS base). **No ARM equivalent** — ARM has no segmentation; per-CPU id comes from `MPIDR_EL1`/`TPIDR_EL1`. |
| **IDT / interrupts** | `kernel/src/interrupts.rs` (53 KB) | `InterruptDescriptorTable` (:17,585), `init_idt()` (:638), `#[x86-interrupt]` handlers, PIC 8259 (`disable_pic` :558, ports 0x21/0xA1), PS/2 ports 0x60/0x64 (:1165,1314,1366), `Cr2` page-fault addr (:975). ARM = exception vector table (`VBAR_EL1`) + GIC, not IDT/PIC. |
| **Context switch** | `kernel/src/context.rs` (entire, 112 lines) | `global_asm!` `switch_context` (callee-saved push/pop, `mov cr3`, `fxsave64`/`fxrstor64`); naked `kernel_thread_entry`, `thread_entry_user` (`iretq`, hardcoded selectors `0x23`/`0x2B`/`0x202`). ARM = save x19-x30+sp, switch `TTBR0_EL1`, `eret`. |
| **Syscall entry** | `kernel/src/syscall.rs:1-90,153-224` | `Efer/LStar/Star/SFMask` MSRs (SYSCALL/SYSRET setup), naked `syscall_handler` with `swapgs`/`sysretq`, `KernelGsBase`. ARM = `SVC` instruction + synchronous-exception handler reading ESR_EL1, `TPIDR_EL1` for per-CPU. |
| **Paging / MM** | `kernel/src/memory.rs` (entire) | `Cr3::read/write` (:78,115,121), `OffsetPageTable`, `PageTable`, `PageTableFlags`, `Mapper`, `PhysFrame`, `Size4KiB`, `VirtAddr`/`PhysAddr` from `x86_64`. ARM = 4-level (or 3) EL1 translation tables, `TTBR0/1_EL1`, `TCR_EL1`, different PTE bit layout (AP/AF/SH/attr-index). |
| **APIC / interrupt controller** | `kernel/src/apic.rs` (24 KB) | `x2apic` crate `LocalApic`/`IoApic`, `enable_x2apic` (MSR 0x1B), LAPIC timer, `calibrate_tsc`, `rdtsc`. ARM = GICv2/GICv3 (distributor + redistributor + CPU interface) + generic timer. |
| **SMP bring-up** | `kernel/src/smp.rs` (+ `smp/` dir) | INIT/SIPI IPI sequence, real-mode trampoline at phys 0x8000, `TRAMPOLINE_SIPI_VECTOR`. ARM = PSCI `CPU_ON` SMC call (no real-mode trampoline). |
| **Timers** | `kernel/src/hpet.rs`, `kernel/src/apic.rs`, `kernel/src/rtc.rs`, `kernel/src/timers.rs` | HPET MMIO, LAPIC-timer, CMOS RTC (ports 0x70/0x71), `rdtsc`. ARM = generic timer (`CNTPCT_EL0`, `CNTP_TVAL_EL0`, `CNTFRQ_EL0`) + PL031 RTC. |
| **MSRs** | `kernel/src/msr.rs`, `msr_amd.rs`, `msr_intel.rs` | `rdmsr`/`wrmsr` asm, CPUID vendor (`__cpuid`), `cpu_family()`. ARM = system registers via `mrs`/`msr`, `MIDR_EL1` for vendor/part. |
| **CPU features / FPU** | `kernel/src/cpu_features.rs` | `cpuid_raw` (:36), `enable_sse` (CR0/CR4, :504), XCR0, `rdtscp`. ARM = `ID_AA64*_EL1` feature regs, SIMD/FP enabled via `CPACR_EL1`. |
| **Port I/O (scattered)** | `interrupts.rs`, `rtc.rs`, `pci.rs`, `serial.rs` | `x86_64::instructions::port::Port` (in/out). ARM has **no port I/O** — PCI config + UART + RTC are all MMIO. `serial.rs` uses `uart_16550` (PIO) — ARM `-M virt` uses a PL011 UART (MMIO). |
| **Misc inline asm** | `main.rs`, `locking.rs`, `scheduler.rs` (50 hits), `panic.rs` | `rdtsc`, `hlt`, `cli`/`sti`, `pause`, `int3`. ARM = `wfi`, `msr daifset/daifclr`, `yield`, `brk`. |

The crate-level dependencies that are **inherently x86**: `x86_64`, `x2apic`, `pic8259`, `uart_16550`, `bootloader_api`/`bootloader`. Each needs an ARM counterpart (`aarch64-cpu`/`cortex-a`, GIC driver, PL011 driver, a different boot protocol).

---

## Prior art & OSS verdict

| System / crate | Mechanism (1-2 lines) | OSS verdict |
|---|---|---|
| **Redox `kernel/src/arch/{x86_64,aarch64,riscv64}/`** | Each arch dir is a sibling module; `arch::CurrentArch` selected by `#[cfg(target_arch)]` at the top of `arch/mod.rs`. Shared kernel calls free functions (`arch::interrupt::*`, `arch::paging::*`) — convention, not a single mega-trait. aarch64 already boots there. | 📖 **study/isolate** — Redox is MIT but per CLAUDE.md §10.13 it is read-only harvest material; copy the *layout pattern* (per-arch module + cfg selection), not code. This spec adopts Redox's directory shape. |
| **Theseus OS** | `kernel/nano_core` per-arch; uses a small `memory`/`interrupts` HAL with arch crates underneath. Boots x86_64; aarch64 in progress. | 📖 study — MIT; pattern only. |
| **`aarch64-cpu` crate** (formerly `cortex-a`) | Typed access to AArch64 system registers (`TTBR0_EL1`, `VBAR_EL1`, `DAIF`, generic timer regs) via `mrs`/`msr` wrappers. The `x86_64`-crate equivalent for ARM. | ➕ **vendorable** (Apache-2.0/MIT) — the recommended ARM register-access crate. |
| **`arm-gic` crate** | GICv2/GICv3 distributor + redistributor + CPU-interface driver. | ➕ vendorable (Apache-2.0/MIT) — candidate for the ARM interrupt-controller backend. Verify exact version before adding. |
| **PSCI (`smccc` crate)** | `SMC`/`HVC` calls for `CPU_ON`/`SYSTEM_OFF`/`PSCI_VERSION` — the ARM SMP + power primitive (no real-mode trampoline). | ➕ vendorable (Apache-2.0/MIT). |
| **edk2 / QEMU `-M virt` ARM firmware (`QEMU_EFI.fd`)** | UEFI on ARM; gives a GOP framebuffer + memory map exactly like x86 UEFI. | external firmware — not vendored; xtask fetches/points at it. |
| **`x86_64` / `x2apic` / `pic8259` / `uart_16550`** (current deps) | The existing x86 backend's register/PIC/UART crates. | **in-use** — keep them; they become the `arch/x86_64` backend's dependencies, NOT shared kernel deps. |
| **rust-osdev `bootloader` crate** | Builds the x86 BIOS/UEFI boot image + `BootInfo`. **x86-only.** | **in-use, x86-only** — stays for x86_64; aarch64 needs a different entry (UEFI stub or QEMU `-kernel`). The `BootInfo` struct must be normalized behind an arch-neutral `BootHandoff` (see Design). |

§R7 / no-Linux-clone: nothing here clones Linux. We harvest the *directory-layout pattern* from Redox (allowed harvest) and use permissive ARM crates. No GPL.

---

## Design

### The boundary: a thin `arch` module, not a god-trait

Redox's lesson (and the right call for a `#![no_std]` kernel) is **convention over a single giant trait object**. A `dyn Arch` trait would force dynamic dispatch on the context-switch and interrupt hot paths — unacceptable per the Concept's latency contracts. Instead:

- `kernel/src/arch/mod.rs` selects the active backend at compile time with `#[cfg(target_arch = …)]` and re-exports it as the `arch` module's public surface.
- The public surface is a set of **free functions, types, and small traits in well-known submodules** (`arch::cpu`, `arch::paging`, `arch::interrupt`, `arch::context`, `arch::time`, `arch::serial`, `arch::smp`, `arch::syscall`). Each backend (`arch/x86_64/`, `arch/aarch64/`, `arch/i686/`) provides identical submodule signatures.
- Generic kernel code calls `crate::arch::paging::map_page(...)`, never `x86_64::...` directly. The compiler monomorphizes to the active backend — **zero runtime cost**, same codegen as today.

```
kernel/src/
  arch/
    mod.rs                 // #[cfg] selects backend; defines the SHARED signatures
                           //   (trait/type aliases the kernel imports), re-exports it
    x86_64/
      mod.rs               // moved-in: gdt, idt-glue, port-io, cr3, swapgs/sysret, x2apic,
      cpu.rs               //   hpet/tsc, smp trampoline. Depends on x86_64/x2apic/pic8259/
      paging.rs            //   uart_16550 crates (now arch-local deps, not workspace deps).
      interrupt.rs
      context.rs           // the global_asm! switch_context lives here
      time.rs
      serial.rs
      smp.rs
      syscall.rs
    aarch64/               // Phase 2+: aarch64-cpu, arm-gic, PL011, PSCI
      mod.rs cpu.rs paging.rs interrupt.rs context.rs time.rs serial.rs smp.rs syscall.rs
    i686/                  // Phase 5: shares much of x86_64 (PIC/PIT/serial) but 32-bit paging
      mod.rs ...
```

### What stays generic (does NOT move into `arch`)

Everything that is logic, not silicon: `scheduler.rs` (policy — EDF/SCHED_GAME), `task.rs` (Task struct, except the saved-register block), `vfs.rs`, `raefs.rs`, `capability.rs`, `compositor.rs`, all the Rae* services, `net*`, `crypto.rs` (already soft-float, arch-neutral after SIMD-gating), ACPI parsing logic (AML is arch-neutral; only the table *discovery* differs — UEFI config table on both x86 and ARM). The scheduler keeps calling `arch::context::switch(...)`; it doesn't know or care about CR3 vs TTBR0.

### The shared signature surface (what `arch/mod.rs` guarantees every backend provides)

These are the seams the inventory above collapses into. Names are the contract; backends implement them.

```
// arch::cpu
fn current_cpu_id() -> usize;          // x86: GS base; arm: TPIDR_EL1
fn halt() -> !;  fn wfi_idle();         // x86: hlt; arm: wfi
fn disable_interrupts() / enable_interrupts() / interrupts_enabled() -> bool;
fn read_timestamp() -> u64;            // x86: rdtsc; arm: CNTPCT_EL0
struct CpuFeatures { … }  fn detect_features() -> CpuFeatures;
fn enable_fp_simd();                   // x86: CR4.OSFXSR; arm: CPACR_EL1

// arch::paging
type PhysAddr; type VirtAddr;          // newtypes (both 64-bit on x86_64/aarch64)
const PAGE_SIZE: usize;
struct AddressSpace { root: PhysAddr } // x86: PML4 frame; arm: TTBR0 base
fn current_address_space() -> AddressSpace;     // x86: Cr3::read
fn switch_address_space(a: &AddressSpace);       // x86: Cr3::write; arm: TTBR0_EL1 + isb
fn map_page(space, virt, phys, flags) -> Result; fn unmap_page(...); fn translate(virt);
struct MapFlags { read, write, exec, user, device } // backend lowers to PTE bits

// arch::interrupt
fn init_bsp();                         // x86: IDT+LAPIC; arm: VBAR_EL1+GIC
fn init_ap(cpu_id);
fn register_handler(vector: u32, f: fn());
fn eoi(vector: u32);
fn fault_address() -> VirtAddr;        // x86: Cr2; arm: FAR_EL1

// arch::context
#[repr(C)] struct Context { /* callee-saved + sp + (cr3|ttbr0) */ }
unsafe fn switch(prev: *mut Context, next: *const Context);
fn new_kernel_thread(entry, stack_top) -> Context;
fn new_user_thread(entry, user_stack, kernel_stack, addr_space) -> Context;

// arch::time
fn init();  fn now_ns() -> u64;  fn busy_wait_us(us: u64);  fn set_oneshot(ns: u64);

// arch::serial
fn init();  fn write_byte(b: u8);      // x86: 0x3F8 PIO; arm: PL011 MMIO

// arch::smp
fn boot_secondary_cpus();              // x86: INIT/SIPI; arm: PSCI CPU_ON

// arch::syscall
fn init_bsp();  fn init_ap(cpu_id);    // x86: LSTAR/STAR/SFMASK; arm: VBAR sync handler
fn set_kernel_stack(cpu_id, top: u64);
```

`kernel_main` becomes arch-neutral: it takes a normalized `BootHandoff { framebuffer, memory_map, rsdp, cmdline }` (constructed by each arch's entry shim from `BootInfo` on x86 or the UEFI/DT handoff on ARM) and drives the existing 9-tier init, replacing every direct `x86_64::`/`gdt::`/`apic::` call with the corresponding `arch::` call.

### Failure modes / decisions (decision-dense)

- **No `dyn`, no trait objects on hot paths.** Compile-time `#[cfg]` selection only → identical codegen to today; this is the entire reason Phase 0 can be a no-regression refactor.
- **`PhysAddr`/`VirtAddr` become kernel-owned newtypes** (in `arch::paging`), not `x86_64::VirtAddr`. This is the single largest mechanical change (every `VirtAddr::new` call site). Mitigation: on x86_64 the newtype is a thin wrapper around `x86_64::VirtAddr` so the backend keeps using the proven crate; the *kernel* just stops importing `x86_64` directly.
- **`BootInfo` normalization:** the rust-osdev bootloader is x86-only and will NOT be the aarch64 entry. Decision: aarch64 boots via **UEFI** (`-M virt` + `QEMU_EFI.fd`) using a small `uefi`-crate stub that builds the same `BootHandoff` (GOP framebuffer + UEFI memory map + ACPI RSDP from the config table) — this keeps ACPI/GOP code shared across both arches, which is worth far more than the cost of a second entry shim. (DT-only boot is rejected: it forks the whole device-discovery path.)
- **Per-CPU id source differs** (GS base vs TPIDR_EL1) but the *value* semantics are identical (0 = BSP) so `scheduler`/`gdt` callers are unaffected once routed through `arch::cpu::current_cpu_id()`.
- **i686 is mostly x86_64 minus long mode:** PIC/PIT/serial/CMOS/CPUID are shared with the x86_64 backend; the deltas are 32-bit paging (2-level + PAE), no SYSCALL/SYSRET on older parts (use `int 0x80`/SYSENTER), and the 32-bit calling convention in `context`. i686 is last because it has the least strategic value (gaming is 64-bit) — it exists to *prove the boundary generalizes to a third arch*, not because anyone ships 32-bit.
- **Security model unchanged:** capability checks, `safe_mode_guard_write`, IOMMU gating are all generic — they sit above `arch`. ARM IOMMU = SMMU (a future arch::iommu backend); the capability layer doesn't change.

---

## Interface needs (NEEDS-INTERFACE)

For **raeen-architect**:

- **None in `rae_abi` for Phase 0.** The arch boundary is an *internal kernel* module surface, not the frozen userspace ABI. Syscall *numbers* are arch-neutral already.
- **Future (Phase 2+):** if aarch64 changes the userspace register-passing convention for syscalls, `rae_abi` may need an arch-tagged calling-convention note — but the *numbers* stay identical. Flag for later; not Phase 0.
- raeen-architect owns the **xtask `--arch=<x86_64|aarch64|i686>` flag** and the per-arch target-triple/QEMU wiring (Build/Tooling section). This is build-system, not ABI, but it is architect's structural commit.

---

## File-by-file plan

### Phase 0 (the unblocking refactor — x86_64 behind the boundary, zero behavior change)
- **NEW `kernel/src/arch/mod.rs`**: `#[cfg(target_arch="x86_64")] pub use x86_64_impl as active;` + the shared submodule re-exports + the signature traits/type-aliases.
- **MOVE** (not rewrite) into `kernel/src/arch/x86_64/`: the bodies of `gdt.rs`, `context.rs`, the IDT/PIC/port parts of `interrupts.rs`, the MSR/SYSCALL parts of `syscall.rs`, `apic.rs`, `hpet.rs`, the `Cr3`/`OffsetPageTable` core of `memory.rs`, `smp.rs` trampoline, `cpu_features.rs`, `serial.rs` (PIO UART). Each becomes `arch/x86_64/<submodule>.rs` exposing the shared signatures; the old top-level file becomes a thin `pub use crate::arch::<submodule>::*;` re-export so **no other file's `use` paths break** in Phase 0.
- **`kernel/src/main.rs`**: keep `entry_point!`/`BootInfo` on x86 inside `arch/x86_64`, have it build a `BootHandoff` and call the (now arch-neutral) `kernel_main(handoff)`. Replace direct `x86_64::` calls in the tier sequence with `arch::` calls incrementally (can be staged).
- **`kernel/Cargo.toml`**: move `x86_64`, `x2apic`, `pic8259`, `uart_16550`, `bootloader_api` under `[target.'cfg(target_arch="x86_64")'.dependencies]` so they don't pull in on aarch64 builds.
- **`Cargo.toml` workspace / `rust-toolchain.toml`**: keep `x86_64-unknown-none` as the only installed target in Phase 0; add `aarch64-unknown-none` only when Phase 2 starts.

### Phase 1 (xtask multi-target wiring — can land parallel with Phase 0)
- **`xtask/src/main.rs`**: add `--arch` flag; map `x86_64`→`x86_64-unknown-none` (default), `aarch64`→`aarch64-unknown-none-softfloat`, `i686`→`i686-unknown-none`. Per-arch QEMU: x86 unchanged; aarch64 → `qemu-system-aarch64 -M virt -cpu cortex-a72 -bios <QEMU_EFI.fd>` (serial via PL011 → `-serial`); image build path forks (UEFI app for ARM vs the bootloader-crate image for x86).

### Phase 2-4 (aarch64 backend — new `raeen-arch` agent)
- **NEW `kernel/src/arch/aarch64/`**: `cpu.rs` (system regs via `aarch64-cpu`), `serial.rs` (PL011 MMIO — first milestone), `paging.rs` (EL1 4-level tables, TTBR0_EL1/TCR_EL1), `interrupt.rs` (VBAR_EL1 vector table + GICv2/v3), `context.rs` (x19-x30/sp save, `eret`), `time.rs` (generic timer), `smp.rs` (PSCI), `syscall.rs` (SVC handler).
- **NEW `kernel/Cargo.toml`** `[target.'cfg(target_arch="aarch64")'.dependencies]`: `aarch64-cpu`, `arm-gic`, `smccc`/PSCI, `uefi`.

### Phase 5 (i686 backend)
- **NEW `kernel/src/arch/i686/`**: reuse x86_64 PIC/PIT/serial/CMOS; new 32-bit `paging.rs` (PAE) + `context.rs` (32-bit ABI) + `int 0x80`/SYSENTER syscall entry.

---

## Build / tooling

| Arch | Target triple | QEMU invocation | Firmware | Image |
|---|---|---|---|---|
| **x86_64** (default) | `x86_64-unknown-none` | `qemu-system-x86_64` (current args) | SeaBIOS / OVMF | rust-osdev `bootloader` BIOS+UEFI img (unchanged) |
| **aarch64** | `aarch64-unknown-none-softfloat` | `qemu-system-aarch64 -M virt -cpu cortex-a72 -m 2G -nographic` | edk2 `QEMU_EFI.fd` (or `-kernel` ELF for the very first "hello") | UEFI app (uefi-crate stub) → ESP, or raw ELF via `-kernel` for Phase 2 first-light |
| **i686** | `i686-unknown-none` | `qemu-system-i386` | SeaBIOS | bootloader-crate BIOS img |

- **x86_64 stays the default** in every command — `xtask build`/`run` with no `--arch` behaves exactly as today (this is a hard requirement; the iron pipeline must not change).
- `--ci` semantics per arch: same "wait for `System successfully booted.`" loop; aarch64 reads the PL011 serial QEMU writes to the same `$TEMP\raeen-serial.log`.
- aarch64 softfloat triple mirrors the kernel's existing soft-float posture (per memory: kernel is `-sse` soft-float on x86; `-softfloat` is the ARM analogue and avoids needing `CPACR_EL1` FP enablement before the first log line).
- First aarch64 bring-up can use `-kernel <elf>` (QEMU loads the ELF, jumps to `_start` at EL1) to get a serial "hello" before the UEFI stub exists — shortest path to a green marker.

---

## Incremental plan (smallest-first, named owners)

| Phase | Goal | Owner | Proof |
|---|---|---|---|
| **0** | Land `arch/` boundary; move x86_64 behind it; **x86_64 boots byte-identically** in QEMU (no-regression). | **raeen-kernel** (the move) + **raeen-architect** (the `arch/mod.rs` signature surface) | x86_64 QEMU boot still prints `[ OS ] System successfully booted.`, all existing smoketest PASS lines unchanged, `[BOOT-BENCH]` not regressed. |
| **1** | xtask `--arch` flag + per-arch target/QEMU wiring; default stays x86_64. | **raeen-architect** | `xtask build` (no flag) unchanged; `xtask build --arch=aarch64` invokes the right cargo target (may fail-to-link until Phase 2 — acceptable). |
| **2** | aarch64 serial "hello": `_start` → `arch::aarch64::serial::init` (PL011) → one marker line, then `wfi`. | **raeen-arch** (new agent) | `qemu-system-aarch64 -M virt` serial shows `[arch:aarch64] PL011 up — RaeKernel first light` then idles (no fault). |
| **3** | aarch64 to MMU + exceptions + timer: enable EL1 paging, install VBAR_EL1, generic-timer tick, GIC EOI. | **raeen-arch** | aarch64 serial shows `[arch:aarch64] MMU+GIC+timer online -> PASS` and survives ≥3 timer ticks without exception loop. |
| **4** | aarch64 to the smoketest set: run the arch-neutral R10 smoketests (crypto KAT, scheduler spawn, vfs) on ARM; reach `System successfully booted.` | **raeen-arch** + **raeen-kernel** | aarch64 QEMU prints `[ OS ] System successfully booted.` + the same generic-module PASS lines that x86 prints. |
| **5** | i686 backend to boot marker (proves the boundary generalizes to a 3rd arch). | **raeen-arch** | `qemu-system-i386` prints `[ OS ] System successfully booted.` |

Phases 0 and 1 are independent and can land together. Phase 2 cannot start until Phase 0 is merged (the `arch/aarch64` submodules must implement the signatures Phase 0 defines).

---

## Acceptance criteria (the exact proof)

- **Phase 0 (the gate):** on `cargo run -p xtask --release -- run --release --ci` (x86_64), serial log MUST still show `[ OS ] System successfully booted.`, MUST NOT show `PANIC`, and every pre-existing smoketest PASS marker (`[msr] run_boot_smoketest … -> PASS`, `[gdt]`, etc.) MUST be unchanged. `[BOOT-BENCH]` total within noise of the pre-refactor number; no new `[boot] WARN`. Per CLAUDE.md §10.17 SMP rule: re-boot ≥5× at `RAEEN_SMP=1` and `=2`. **A diff that changes any boot-log line other than nothing has regressed.**
- **Phase 2:** `qemu-system-aarch64 -M virt` serial MUST show `[arch:aarch64] PL011 up — RaeKernel first light` and the VM MUST NOT enter an exception loop (no repeating fault address).
- **Phase 3:** aarch64 serial MUST show `[arch:aarch64] MMU+GIC+timer online -> PASS` with the assertion that ≥3 generic-timer interrupts were taken and EOI'd.
- **Phase 4:** aarch64 serial MUST show `[ OS ] System successfully booted.` plus the arch-neutral KAT PASS lines (crypto, scheduler).
- **`/proc/raeen/arch` MUST report:** `arch: <x86_64|aarch64|i686>`, `cpus_online: <n>`, `page_size: <bytes>`, `interrupt_controller: <APIC|GICv3|…>`, `timer: <TSC+LAPIC|generic>`. (New procfs line in `vfs.rs`, arch-neutral, populated from `arch::cpu`/`arch::time`.)
- **Docstring:** `arch/mod.rs` MUST quote the §Thesis "third path / locked behind Apple silicon" promise above.

---

## Handoff

- **First commit (raeen-architect, structural — NOT `[interface]`/`rae_abi`):**
  `arch: introduce kernel/src/arch boundary (x86_64 behind it, no behavior change)`
  - Create `kernel/src/arch/mod.rs` with the `#[cfg(target_arch="x86_64")]` selection + the shared submodule signature surface (the trait/type-alias list under "Design").
  - Create `kernel/src/arch/x86_64/{mod,cpu,paging,interrupt,context,time,serial,smp,syscall}.rs` as **re-export shells** that initially `pub use` the existing top-level modules (`pub use crate::context::*;` etc.) — this lands the *directory + signature surface* with **zero code motion and zero behavior change**, so x86_64 boots byte-identically and the no-regression gate passes on the very first commit.
  - The actual *body relocation* (moving `gdt.rs`/`context.rs`/etc. into `arch/x86_64/`) is the **second** commit (raeen-kernel), done one module at a time, each re-verified against the Phase-0 gate.
  - Move the x86-only crates under `[target.'cfg(target_arch="x86_64")'.dependencies]` in `kernel/Cargo.toml` in this same first commit (proves the workspace still builds with arch-gated deps).
  - **Acceptance for this commit:** `cargo run -p xtask --release -- build --release` exits 0; `--ci` boot still prints `System successfully booted.` with no new/changed/missing log lines; ≥5 boots clean at SMP=1 and SMP=2.

- **Implementer:** raeen-kernel (Phase 0 body moves + per-arch x86_64 core), raeen-architect (Phase 0 `arch/mod.rs` signatures + Phase 1 xtask), new **raeen-arch** agent (Phases 2-5 aarch64/i686 bring-up).
- **Unblocks checklist lines:** net-new — add a MasterChecklist section "Phase 15: Multi-arch reach" with lines 15.0 (boundary), 15.1 (xtask --arch), 15.2 (aarch64 hello), 15.3 (aarch64 MMU/GIC/timer), 15.4 (aarch64 boot marker), 15.5 (i686 boot marker). Also unblocks the long-deferred line 936 (ARM MTE) since MTE can only land once an aarch64 backend exists.
- **Sequencing:** Phase 0 first commit (re-export shells) → Phase 0 body moves → Phase 1 xtask (parallel-safe) → Phase 2+ aarch64. No `rae_abi` bump required for Phase 0; revisit ABI only if aarch64 syscall convention diverges.

---

## Open questions for the lead

1. **aarch64 entry: UEFI stub vs DT/`-kernel`?** This spec recommends UEFI (shares ACPI/GOP with x86); confirm before raeen-arch builds the entry shim. Cheapest first-light is `-kernel` ELF, then graduate to UEFI.
2. **Is i686 worth it at all?** It proves the boundary generalizes but ships to no gamer. Could be dropped to "boundary-ready, not built" and the proof-of-generality deferred to a RISC-V backend instead (more strategically interesting for the Rae Station).
3. **Newtype `VirtAddr`/`PhysAddr` blast radius:** wrapping `x86_64::VirtAddr` keeps Phase 0 mechanical, but the call-site count is large. Acceptable to stage the newtype migration across several commits behind the Phase-0 gate?
