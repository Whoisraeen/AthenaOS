# Spec: aarch64 (ARM64) Bring-Up Plan

## Concept promise served

> "Windows became bloated chasing enterprise. macOS got locked behind a walled garden of Apple silicon. Linux never figured out gaming or design coherence. AthenaOS is the third path…" (§Thesis)

> "Memory tagging on supported CPUs (ARMv8.5 MTE, Intel/AMD equivalent as it ships)" (§Security Model)

The credibility of "the answer to macOS being locked to Apple silicon" requires that AthenaOS itself run on ARM. Goal #3 (owner directive) makes this falsifiable: **AthenaOS must boot + run + install on x86_64 (done), aarch64, and i686 — each proven independently in QEMU.** aarch64 is at 0%. This document is the *executable* version of the prior abstraction spec — it turns the boundary design into a staged, buildable sequence with the exact boot-log line that proves each step.

This is **research only**. It writes no kernel code. It builds directly on `docs/research/arch-abstraction-layer.md` (the Phase-0 `arch/` boundary spec) and does not re-derive it — read that first. Where this doc says "the boundary," it means the `kernel/src/arch/{x86_64,aarch64,i686}/` + `arch/mod.rs` structure defined there.

---

## Relationship to the prior spec (do not duplicate)

`docs/research/arch-abstraction-layer.md` already specified:

- The `arch/` directory layout and `#[cfg(target_arch)]` compile-time backend selection (no `dyn`, no hot-path vtables).
- The shared signature surface: `arch::{cpu, paging, interrupt, context, time, serial, smp, syscall}`.
- The `BootHandoff` normalization (replaces the x86-only `bootloader_api::BootInfo`).
- The migration order: Phase 0 re-export shells → body moves → xtask `--arch` → aarch64 backend.

**This doc owns the aarch64-specific delta:** the per-seam x86→ARM mechanism table, the exact target/QEMU/firmware wiring (verified against this machine), the smallest-provable-increment stage ladder with literal boot-log strings, and the ADR-worthy ACPI-vs-DeviceTree decision. It assumes the boundary from the prior spec is the substrate.

---

## Already in the tree (verify-before-implement) — confidence-rated

Re-verified against the current tree on 2026-06-17. Confidence column: **V** = I read the code/file directly this session; **I** = inferred from a grep/import without reading the full body.

| Fact | Status | Conf |
|---|---|---|
| No `kernel/src/arch/` directory exists (`glob kernel/src/arch/**` → empty) | `[ ]` nothing built | **V** |
| `rust-toolchain.toml` installs only `targets = ["x86_64-unknown-none"]` | x86-only | **V** |
| `.cargo/config.toml` has only a `[target.x86_64-unknown-none]` rustflags block (frame-pointers + curve25519 fiat backend) | x86-only | **V** |
| `kernel/src/main.rs:301` `use bootloader_api::{…BootInfo, BootloaderConfig}` + `:312 entry_point!` — x86-only rust-osdev bootloader | x86-only entry | **V** |
| `kernel_main` (main.rs:330) takes `&'static mut BootInfo`, first op is a raw `rdtsc` (main.rs:334-345); SSE enable (`cpu_features::enable_sse`, main.rs:396) | x86-specific entry preamble | **V** |
| `xtask/src/main.rs` hardcodes `--target x86_64-unknown-none` in 5 places (lines 184, 226, 259, 287, and the relibc path 278/535/1207) | x86-only build | **V** |
| `xtask` CI exit uses `-device isa-debug-exit,iobase=0xf4` (main.rs:827) — **x86 port-I/O device, has no aarch64 equivalent** | needs ARM CI-exit path | **V** |
| `xtask` UEFI path maps OVMF as `if=pflash` (main.rs:686-704); firmware search at `find_ovmf()` (main.rs:1058) is x86 OVMF only | needs aarch64 firmware path | **V** |
| QEMU acceleration logic (main.rs:776-804) is generic (TCG/KVM/WHPX) but always invokes `qemu-system-x86_64` (`find_qemu()` main.rs:1106) | needs `qemu-system-aarch64` | **V** |
| **`qemu-system-aarch64.exe` IS present** at `C:\Program Files\qemu\` | available | **V** |
| **`edk2-aarch64-code.fd` IS present** at `C:\Program Files\qemu\share\` (also `edk2-arm-code.fd` / `edk2-arm-vars.fd`) | available | **V** |
| MasterChecklist has NO aarch64/arm64/multi-arch section. Phase 15 = "AthStore/AthID/AthSync"; highest phase is **Phase 19** (Accessibility). Only ARM mention: a deferred MTE line. | net-new section | **V** |
| `kernel/Cargo.toml` deps `x86_64`, `x2apic`, `pic8259`, `uart_16550`, `bootloader_api` are unconditional (not `[target.'cfg(...)']`-gated) | must be arch-gated | **I** (per prior spec; not re-read this session) |

**Correction to the prior spec:** it proposed adding "Phase 15: Multi-arch reach." Phase 15 is already taken by AthStore/AthID/AthSync. **This work is Phase 20.** (See Handoff.)

---

## 1. INVENTORY — the x86→ARM seam table

Every row maps to a submodule of the boundary from the prior spec. "Seam exists?" = whether the prior abstraction spec already names a signature for it (it does for all of these — none require *new* boundary surface, only an aarch64 *implementation* of the existing surface). Confidence as above.

| Seam (`arch::` submodule) | What x86_64 does today (file) | aarch64 equivalent | Seam already specced? | Conf |
|---|---|---|---|---|
| **Entry / boot info** (`arch::<entry>` + `BootHandoff`) | `bootloader_api::entry_point!` → `&mut BootInfo` (`main.rs:301,312,330`); rust-osdev bootloader builds the BIOS/UEFI image | No rust-osdev support. Two options: (a) `-kernel <ELF>` → QEMU jumps to `_start` at EL1 (first-light), (b) a small `uefi`-crate EFI app on the `-bios edk2-aarch64-code.fd` ESP that builds the same `BootHandoff`. | Yes (`BootHandoff`) | **V** |
| **Serial** (`arch::serial`) | `uart_16550` PIO at port `0x3F8` (`serial.rs`); first log line `[ OK ] Serial (COM1 16550 UART) @ 0x3F8` (main.rs:365) | **PL011** UART, MMIO. On `qemu-system-aarch64 -M virt` the PL011 is at **`0x0900_0000`** (the `virt` machine's UART0). Pre-MMU it is identity/flat-addressable; write `DR` (offset 0x00), poll `FR.TXFF` (offset 0x18). | Yes | **V** |
| **CPU primitives** (`arch::cpu`) | `rdtsc` (main.rs:334), `hlt`, `cli`/`sti`, GS-base per-CPU id (`gdt.rs`), CPUID (`cpu_features.rs`) | `CNTPCT_EL0` (timestamp), `wfi` (halt), `msr daifset/daifclr #2` (mask IRQ), `TPIDR_EL1` (per-CPU id), `MIDR_EL1`/`ID_AA64*_EL1` (feature/vendor) — all via `aarch64-cpu` crate | Yes | **V** |
| **Exception model** (`arch::interrupt`) | IDT (`InterruptDescriptorTable`, interrupts.rs:17), PIC 8259 (`disable_pic`, ports 0x21/0xA1), `#[x86-interrupt]` handlers, `Cr2` fault addr (interrupts.rs:975) | **VBAR_EL1** points at a 2 KiB-aligned, 16-entry **exception vector table** (4 groups × {sync, IRQ, FIQ, SError}). No IDT. Fault address = `FAR_EL1`; cause = `ESR_EL1` (EC field). | Yes | **V** |
| **Interrupt controller** (`arch::interrupt`) | x2APIC LAPIC + IOAPIC (`apic.rs`, `x2apic` crate); EOI via MSR | **GIC**. `virt` machine = **GICv3** by default (selectable v2). Distributor (`GICD`) at `0x0800_0000`, redistributors (`GICR`) at `0x080A_0000`; CPU interface via system regs (`ICC_*_EL1`). EOI = `ICC_EOIR1_EL1`. | Yes | **V** |
| **Paging / MMU** (`arch::paging`) | `Cr3::read/write` (memory.rs:78,115), `OffsetPageTable`, 4-level PML4, PTE flags from `x86_64` crate | **TTBR0_EL1** (user/low) + **TTBR1_EL1** (kernel/high), config in **TCR_EL1**, attributes in **MAIR_EL1**. 4 KiB granule, 4-level (48-bit VA). PTE bits differ entirely: AP[2:1], AF, SH[1:0], attr-index, UXN/PXN, table vs block descriptor. `dsb`+`isb`+`tlbi` for invalidation. | Yes | **V** |
| **Context switch** (`arch::context`) | `global_asm!` `switch_context` (context.rs): push/pop callee-saved, `mov cr3`, `fxsave64`; `iretq`/`sysretq` with hardcoded selectors | Save **x19–x30** + **sp** (+ FP/SIMD if used), switch **TTBR0_EL1** + `isb`, return via **`eret`** with SPSR/ELR set. No segment selectors. | Yes | **V** |
| **Syscall entry** (`arch::syscall`) | `Efer/LStar/Star/SFMask` MSRs, naked `syscall_handler` with `swapgs`/`sysretq` (syscall.rs:1-90) | **`SVC #0`** instruction → synchronous exception (same VBAR_EL1 sync vector). Handler reads `ESR_EL1.EC == 0b010101` (SVC), args in x0–x7, number in x8 (or w8), returns in x0, `eret`. Per-CPU via `TPIDR_EL1` (no swapgs). | Yes | **V** |
| **SMP bring-up** (`arch::smp`) | INIT/SIPI IPI + real-mode trampoline at phys 0x8000 (`smp.rs`) | **PSCI** `CPU_ON` via `SMC #0` (or `HVC` under a hypervisor); secondaries enter at a passed `entry_point_address` already in EL1. No real-mode, no trampoline. `smccc`/PSCI crate. | Yes | **V** |
| **Timers** (`arch::time`) | HPET MMIO + LAPIC timer + `rdtsc` calibration (`hpet.rs`, `apic.rs`), CMOS RTC ports 0x70/0x71 (`rtc.rs`) | **ARM generic timer**: `CNTFRQ_EL0` (freq, fixed — no calibration needed), `CNTPCT_EL0` (counter), `CNTP_TVAL_EL0`/`CNTP_CTL_EL0` (per-CPU oneshot), routed as PPI 30/27 through the GIC. RTC = **PL031** MMIO (`0x0901_0000` on virt). | Yes | **V** |
| **Port I/O (scattered)** | `x86_64::instructions::port::Port` in interrupts.rs (PS/2 0x60/0x64), rtc.rs, pci.rs, serial.rs | **No port I/O on ARM.** PCIe config = **ECAM** MMIO (virt: `0x4010_0000`). PS/2 N/A (USB-HID only). This forces port-I/O call sites behind `arch::io` or to be feature-gated off on ARM. | Partial — prior spec folds this into per-backend serial/pci; **add `arch::io` note** | **V** |
| **FP/SIMD enable** (`arch::cpu`) | `enable_sse` sets CR4.OSFXSR/OSXMMEXCPT (main.rs:396, cpu_features.rs) | Set **`CPACR_EL1.FPEN = 0b11`** to not-trap FP/SIMD at EL0/EL1. With the `-softfloat` target this is deferrable past first-light (kernel is soft-float — see memory `kernel-soft-float-no-fpu-save`). | Yes | **V** |
| **CI exit device** (xtask) | `-device isa-debug-exit,iobase=0xf4` (x86 port write → QEMU exit, main.rs:827) | **No isa-debug-exit on `virt`.** ARM CI exit = the **`semihosting`** `SYS_EXIT` call (`-semihosting-config enable=on,target=native`) via `hlt #0xf000`, OR poll the serial log for `System successfully booted.`/`PANIC` and kill QEMU (the existing CI loop already does the latter — preferred, zero kernel cost). | No — **xtask delta** | **V** |

**Net:** every *kernel-internal* seam already has a named signature in the prior spec — aarch64 needs an *implementation*, not new boundary surface. The two genuinely-new items are build-system: (1) a generalized `arch::io` so port-I/O call sites compile on ARM (or get `#[cfg]`-stubbed), and (2) the xtask CI-exit mechanism for `virt`.

---

## 2. TARGET + TOOLCHAIN (verified against this machine)

| Item | Value | Verified |
|---|---|---|
| Target triple | **`aarch64-unknown-none-softfloat`** | softfloat mirrors the kernel's existing `-sse`/soft-float posture on x86 (memory `kernel-soft-float-no-fpu-save`); avoids needing `CPACR_EL1` FP-enable before the first log line. The hardfloat `aarch64-unknown-none` is the later option once FP is wired. |
| Toolchain | `rust-toolchain.toml` → add `aarch64-unknown-none-softfloat` to `targets` (keep `x86_64-unknown-none` first/default) | nightly + `rust-src` already present (used for `build-std`) |
| QEMU binary | **`qemu-system-aarch64`** | present at `C:\Program Files\qemu\` |
| QEMU machine | **`-M virt -cpu cortex-a72`** | `cortex-a72` = a stable A-class core QEMU models well; `virt` is the para-virt board with well-known MMIO map (PL011 0x0900_0000, GICv3, ECAM 0x4010_0000) |
| GIC version | `-M virt` defaults to **GICv3**; force with `gic-version=3` (or `=2` if v3 init is harder to bring up first) | recommend pinning `gic-version=3` for determinism |
| Firmware (UEFI path) | **`edk2-aarch64-code.fd`** mapped `if=pflash,readonly=on` (+ a writable `vars` pflash) | present in QEMU share dir; mirrors the existing x86 OVMF `if=pflash` handling (main.rs:696) |
| First-light boot (Stage 1–3) | **`-kernel target/aarch64-unknown-none-softfloat/release/kernel`** — QEMU loads the ELF and jumps to `_start` at EL1, no firmware needed | the shortest path to a serial marker; defer UEFI to Stage 4 |
| Serial | PL011 → `-serial file:$TEMP/athena-serial.log` (the SAME path the x86 CI loop reads) | xtask already writes there (main.rs:810) |
| CI exit | poll the serial log for the success/PANIC marker and kill QEMU (no isa-debug-exit on virt) | the existing `--ci` loop already does marker-polling; ARM just lacks the *extra* hard-exit device |

### Exact xtask changes (Phase 1 of the prior spec, made concrete)

`xtask/src/main.rs`:

1. **`--arch=<x86_64|aarch64|i686>` flag**, default `x86_64`. Maps to triple: `x86_64`→`x86_64-unknown-none`, `aarch64`→`aarch64-unknown-none-softfloat`, `i686`→`i686-unknown-none`.
2. **`build_kernel`** (main.rs:181): replace the hardcoded `--target x86_64-unknown-none` (line 184) with the arch-mapped triple. (Leave `build_user_apps`/relibc on x86_64 for now — userspace multi-arch is out of scope until Stage 5+.)
3. **`find_qemu`** (main.rs:1106): when arch≠x86_64, return `qemu-system-aarch64`.
4. **`run_qemu`** (main.rs:677): for aarch64, replace the x86 arg block with `-M virt -cpu cortex-a72 -m 2G -smp <n> -nographic`. For Stage 1–3 use `-kernel <elf>`; for Stage 4+ add `-drive if=pflash,readonly=on,file=edk2-aarch64-code.fd` + a vars pflash and an ESP image. **Drop `-device isa-debug-exit`** on aarch64 (rely on marker-poll exit).
5. **`find_ovmf`** (main.rs:1058): add an aarch64 firmware search returning `edk2-aarch64-code.fd` (the Windows path `C:\Program Files\qemu\share\edk2-aarch64-code.fd` is confirmed present; add the Linux `/usr/share/AAVMF/` paths too for WSL2/KVM).
6. **x86 default is untouched** — `xtask build`/`run` with no `--arch` must behave byte-identically (hard requirement; the iron pipeline must not change).

### Cargo / deps gating

- `kernel/Cargo.toml`: move `x86_64`, `x2apic`, `pic8259`, `uart_16550`, `bootloader_api` under `[target.'cfg(target_arch="x86_64")'.dependencies]`; add `[target.'cfg(target_arch="aarch64")'.dependencies]` = `aarch64-cpu`, `arm-gic`, `smccc` (PSCI), and (Stage 4) `uefi`.
- `.cargo/config.toml`: add a `[target.aarch64-unknown-none-softfloat]` block. It needs a linker script for the `-kernel` ELF (load address, `_start` at the entry, BSS/stack) — `aarch64` `-kernel` loads at `0x4008_0000` on `virt`. Mirror the frame-pointer rustflag.

---

## 3. STAGED PLAN (smallest-provable-increments)

Each stage names the exact serial line that proves it. **x86_64 stays GREEN at every stage** — none of these stages touch the x86 backend after the Phase-0 boundary lands. Stages 0–1 are prerequisites from the prior spec; this doc owns Stages S1–S6 (aarch64).

| Stage | Goal | Implementer | The boot-log line that proves it |
|---|---|---|---|
| **S0** | Land the `arch/` boundary (re-export shells → body moves); **x86_64 boots byte-identically**. | athena-architect (mod.rs surface) + athena-kernel (moves) | x86 QEMU still prints `[ OS ] System successfully booted.`, all PASS lines unchanged, `[BOOT-BENCH]` not regressed. *(This is Phase 0 of the prior spec — its gate is the prerequisite for everything below.)* |
| **S0.1** | xtask `--arch` + per-arch target/QEMU/firmware wiring; default stays x86_64. | athena-architect | `xtask build` (no flag) unchanged; `xtask build --arch=aarch64` invokes the right triple (may fail-to-link until S1 — acceptable). |
| **S1** | **First light.** `_start` (EL1) → `arch::aarch64::serial::init` (PL011 @ 0x0900_0000) → one marker → `wfi` loop. No MMU, no exceptions, no heap. Built `--target aarch64-unknown-none-softfloat`, run with `-kernel`. | **athena-arch** | `qemu-system-aarch64 -M virt` serial shows **`[arch:aarch64] PL011 up — AthKernel first light (EL1)`** then idles — no repeating fault, no reset. |
| **S2** | **MMU + heap.** Build EL1 translation tables (identity-map the PL011 + RAM, set TCR_EL1/MAIR_EL1/TTBR1_EL1, enable SCTLR_EL1.M), then init the existing linked-list heap on the ARM memory map. | athena-arch | serial shows **`[arch:aarch64] MMU on (TTBR1_EL1, 4KiB/48-bit) — heap up`** and a post-MMU `alloc` test line (`[arch:aarch64] heap alloc/free -> PASS`). |
| **S3** | **Exceptions + timer + GIC.** Install VBAR_EL1 vector table; init GICv3 (distributor + redistributor + ICC sysregs); program the generic timer (CNTP_TVAL_EL0) for a periodic tick; take + EOI ≥3 ticks. | athena-arch | serial shows **`[arch:aarch64] VBAR+GICv3+generic-timer online -> PASS (ticks=3)`** and survives without an exception loop (no repeating `ESR_EL1`/`FAR_EL1`). |
| **S4** | **Arch-neutral smoketests + boot marker.** Build via UEFI (edk2-aarch64-code.fd → `BootHandoff` from GOP + UEFI memmap + ACPI RSDP). Run the generic R10 smoketests that are silicon-independent (crypto KAT, scheduler spawn, vfs, athfs in-memory) and reach the success marker. | athena-arch + athena-kernel | serial shows **`[ OS ] System successfully booted.`** plus the SAME generic-module PASS lines x86 prints (`[crypto] … -> PASS`, `[sched] … -> PASS`). `--ci` exits 0 via marker-poll. |
| **S5** | **Install on aarch64** (Goal #3's third verb). GPT + ESP + AthFS mkfs on a QEMU virtio-blk disk, write→readback under the safe-mode guard, reach the installer marker. | athena-arch + athena-fs | serial shows the existing installer proof line on aarch64 (`[installer] AthFS mkfs + ESP written -> PASS`) — proving boot+run+**install** all three on ARM. |
| **S6** *(stretch)* | **SMP + desktop.** PSCI `CPU_ON` for secondaries; reach the compositor/desktop. (Deferred — see Risks; not required for the Goal #3 "boot+run+install" bar.) | athena-arch | `[smp] aarch64 N CPUs online via PSCI -> PASS`; later a composited frame. |

**Sequencing:** S0 gates everything. S1→S2→S3 are strictly serial (each needs the prior). S4 needs S3 (timer/interrupts for the scheduler) + the UEFI entry. S5 needs S4. S6 is independent stretch after S4.

---

## 4. ARCH-ABSTRACTION MECHANISM (how x86 stays GREEN)

This restates the prior spec's mechanism only where it bears on the aarch64 migration order. **Read the prior spec for the full `arch/mod.rs` signature surface.**

- **Compile-time selection, zero hot-path cost.** `arch/mod.rs` does `#[cfg(target_arch="aarch64")] pub use self::aarch64 as imp;` etc. Generic kernel code calls `crate::arch::paging::map_page(...)`; the compiler monomorphizes to the active backend — identical codegen to today on x86. No `dyn Arch`, no vtable on context-switch/interrupt paths (the Concept latency contracts forbid it).
- **`kernel_main` becomes arch-neutral.** It takes `BootHandoff { framebuffer, memory_map, rsdp, cmdline }`. Each arch's entry shim builds it: x86 from `bootloader_api::BootInfo`; aarch64 from either the `-kernel` register state (Stage 1–3: hardcoded `virt` map) or the UEFI handoff (Stage 4+). The 9-tier init sequence in `main.rs` stays put; the `rdtsc` preamble (main.rs:334) and `enable_sse` (main.rs:396) move into `arch::x86_64`'s entry, replaced by `arch::cpu::read_timestamp()` / `arch::cpu::enable_fp_simd()`.
- **Migration order that never breaks x86 (the critical constraint):**
  1. **Boundary lands as re-export shells** — `arch/x86_64/*.rs` initially just `pub use crate::<oldmod>::*;`. Zero code motion, zero behavior change, x86 boots byte-identically. *(prior spec, first commit.)*
  2. **Bodies move one module at a time**, each re-verified against the S0 gate (x86 boot unchanged, ≥5 boots SMP=1 and SMP=2 per CLAUDE.md §10.17).
  3. **aarch64 backend is purely additive** — it lives in `arch/aarch64/` and only compiles under `cfg(target_arch="aarch64")`. **It can never regress an x86 build** because the x86 build never compiles it. This is the safety property that lets athena-arch iterate freely on ARM while x86 iron work continues in parallel.
- **The `arch::io` addition** (new vs prior spec): port-I/O call sites (`x86_64::instructions::port::Port`) that are NOT inside an already-moved backend module need either an `arch::io` shim (x86 = real PIO, aarch64 = `unreachable!()`/MMIO ECAM) or a `#[cfg(target_arch="x86_64")]` guard. Inventory the residual `Port` uses during S0 body-moves and route them; this is the one place the boundary surface grows.

---

## 5. RISKS + what's hard

| Risk / hard part | Why | Mitigation / decision |
|---|---|---|
| **rust-osdev `bootloader` crate is x86-only** | It builds the BIOS/UEFI image + `BootInfo`. There is no ARM analogue. | Stages S1–S3 use QEMU `-kernel <ELF>` (no bootloader at all). S4 uses a small `uefi`-crate EFI app on edk2-aarch64 firmware → `BootHandoff`. The bootloader crate stays x86-only forever; it's behind the boundary. |
| **ACPI vs DeviceTree on ARM** (ADR-worthy) | `qemu -M virt` provides BOTH a DTB (in `x0` at boot) and ACPI tables (when booted via UEFI). The kernel's whole device-discovery path (`acpi_full.rs`, the vendored `aml` crate, `_PRT` IRQ routing) is ACPI. Re-targeting it to DT forks device discovery. | **Recommend UEFI + ACPI on aarch64** (see ADR below). DTB is only the cheap source of the `virt` MMIO base addresses for the bring-up stages (S1–S3 can hardcode the well-known `virt` map instead of even parsing the DTB). |
| **GICv3 init is finicky** | Distributor + per-CPU redistributor + system-register CPU interface, with a strict enable order and `dsb`/`isb` barriers. A wrong order = no interrupts (silent) or an SError. | Pin `gic-version=3`. Bring up GIC in S3 *after* MMU (S2) so MMIO is mapped. Use the `arm-gic` crate (handles the redistributor walk) rather than hand-rolling. Have a fallback to `gic-version=2` if v3 stalls. |
| **No isa-debug-exit on `virt`** | The x86 CI relies on a port write to exit QEMU with a code. | Use the existing marker-poll CI loop (already present) + kill QEMU; optionally enable `-semihosting` `SYS_EXIT` later for a clean exit code. No kernel-side dependency. |
| **SMP / real drivers deferral** | PSCI SMP, virtio drivers, USB, GPU on ARM are large. Goal #3 only requires boot+run+install. | **Defer SMP to S6** (boot BSP-only; `-smp 1`). **Defer real device drivers** — S4/S5 use virtio-blk (the kernel already has `virtio`/`virtio_net`/`virtio_gpu`, which are MMIO and largely arch-neutral). USB/AMD-GPU on ARM is out of scope until after Goal #3 is met. |
| **FP/SIMD trap** | If any kernel path uses NEON before `CPACR_EL1.FPEN` is set, it traps. | `-softfloat` target keeps the kernel off NEON (matches the x86 soft-float posture). Set `CPACR_EL1.FPEN` in S2/S4 only when userspace/relibc needs it. |
| **Linker script / load address** | `-kernel` on `virt` loads at `0x4008_0000`; the ELF needs a matching linker script + early stack before `_start` can call Rust. | Provide an aarch64 linker script in `.cargo` / a `build.rs`; `_start` sets `sp`, zeroes BSS, then `bl kernel_entry`. Standard aarch64 baremetal preamble — harvest the *pattern* from Redox `arch/aarch64/` (read-only, MIT, layout-only). |

### ADR-worthy decision: UEFI + ACPI on aarch64 (recommended)

**Decision (for a later formal ADR):** boot aarch64 via **UEFI** (edk2-aarch64-code.fd) and discover devices via **ACPI**, not DeviceTree.

**Rationale:**
1. **Maximum code reuse.** The kernel's device discovery is already ACPI (`acpi_full.rs` + vendored `aml`). The `arm64 SBBR`/`SBSA` server-platform standard mandates ACPI+UEFI, and QEMU `virt`-via-UEFI supplies ACPI tables. Reusing the iron-proven AML parser (0→159 devices on Athena) across both arches is worth far more than a second discovery path.
2. **GOP + memory map come free.** UEFI gives the same GOP framebuffer + memory-map handoff as x86 UEFI, so `BootHandoff` construction is near-identical across arches — the compositor/framebuffer code doesn't fork.
3. **Real ARM laptops/handhelds (the Concept's target) ship UEFI+ACPI**, not bare DT — so this matches where AthenaOS actually wants to run (the anti-"locked to Apple silicon" pitch). Bare-DT is the embedded/SoC world AthenaOS is not chasing.

**Cost / counter-argument:** DTB is simpler for the `virt` board alone and is handed to us free in `x0`. **Accommodation:** Stages S1–S3 (`-kernel`) hardcode the well-known `virt` MMIO bases (PL011 0x0900_0000, GICD 0x0800_0000, ECAM 0x4010_0000) — no DTB parse needed — and only S4 onward goes through UEFI+ACPI. So we get the cheap bring-up without forking discovery, and the production path is ACPI. **Rejected:** a DT-primary discovery path (forks `acpi_full.rs` and the device model for no strategic gain).

---

## Interface needs (NEEDS-INTERFACE)

For **athena-architect**:

- **None in `ath_abi` for any of S0–S5.** The arch boundary is an internal kernel module surface; syscall *numbers* are arch-neutral. (Per the prior spec, revisit `ath_abi` only if the aarch64 *userspace* syscall calling convention diverges — not relevant until userspace runs on ARM, post-S5.)
- athena-architect owns the **xtask `--arch` flag** + per-arch target/QEMU/firmware wiring (Section 2). Structural commit, not `[interface]`.
- New (vs prior spec): the **`arch::io` shim** signature (x86 PIO / aarch64 no-op) — internal `arch` surface, architect to fold into `arch/mod.rs`.

---

## File-by-file plan (aarch64 delta only — see prior spec for S0)

- **`rust-toolchain.toml`**: add `aarch64-unknown-none-softfloat` to `targets` (keep x86 first).
- **`.cargo/config.toml`**: add `[target.aarch64-unknown-none-softfloat]` (rustflags + linker script ref).
- **`xtask/src/main.rs`**: `--arch` flag; arch-mapped triple in `build_kernel`; `find_qemu`→`qemu-system-aarch64`; `run_qemu` aarch64 arg block (`-M virt -cpu cortex-a72`, `-kernel` for S1–S3, pflash firmware for S4+, drop isa-debug-exit); `find_ovmf` aarch64 firmware search (`edk2-aarch64-code.fd`).
- **`kernel/Cargo.toml`**: arch-gate x86 crates; add aarch64 deps (`aarch64-cpu`, `arm-gic`, `smccc`, later `uefi`).
- **NEW `kernel/src/arch/aarch64/`**: `entry.rs` (`_start`, BSS/stack, `BootHandoff`), `serial.rs` (PL011 — **S1**), `cpu.rs` (system regs via `aarch64-cpu`), `paging.rs` (TTBR/TCR/MAIR, 4-level — **S2**), `interrupt.rs` (VBAR + GICv3 — **S3**), `time.rs` (generic timer — **S3**), `context.rs` (x19–x30/sp + `eret`), `smp.rs` (PSCI — S6), `syscall.rs` (SVC).
- **NEW** aarch64 linker script (in `kernel/` or `.cargo`), load `0x4008_0000`, `_start` entry.
- **MasterChecklist.md**: add **Phase 20 — Multi-arch reach** (see Handoff).

---

## Acceptance criteria (the exact proof)

- **S1 (first light):** `qemu-system-aarch64 -M virt` serial MUST show `[arch:aarch64] PL011 up — AthKernel first light (EL1)` and the VM MUST NOT enter an exception loop (no repeating `ESR_EL1`/`FAR_EL1`, no reset).
- **S2:** serial MUST show `[arch:aarch64] MMU on (TTBR1_EL1, 4KiB/48-bit) — heap up` + `[arch:aarch64] heap alloc/free -> PASS`.
- **S3:** serial MUST show `[arch:aarch64] VBAR+GICv3+generic-timer online -> PASS (ticks=3)` asserting ≥3 timer IRQs taken + EOI'd.
- **S4:** serial MUST show `[ OS ] System successfully booted.` plus the arch-neutral KAT PASS lines (`[crypto] … -> PASS`, `[sched] … -> PASS`); `--ci` MUST exit 0.
- **S5:** serial MUST show `[installer] AthFS mkfs + ESP written -> PASS` on aarch64 (boot+run+install, all three, proving Goal #3 on ARM).
- **`/proc/athena/arch` MUST report** (new arch-neutral procfs line in `vfs.rs`, populated from `arch::cpu`/`arch::time`): `arch: aarch64`, `cpus_online: <n>`, `page_size: 4096`, `interrupt_controller: GICv3`, `timer: generic`.
- **x86 no-regression at every stage:** the x86 CI boot MUST be byte-identical (S0 gate) — aarch64 stages cannot change it because the x86 build never compiles `arch/aarch64`.
- **Docstring:** `arch/aarch64/mod.rs` MUST quote the §Thesis "third path / locked behind Apple silicon" promise.

---

## Handoff

- **Named hand-off — who starts Stage 1:** the **athena-arch** agent (new identity, ARM/i686 bring-up), **after** athena-architect + athena-kernel land S0 (the `arch/` boundary) and S0.1 (xtask `--arch`). athena-arch cannot start S1 until `xtask build --arch=aarch64` produces an ELF.
- **First athena-arch commit (S1):** `arch(aarch64): PL011 first-light — _start, serial, wfi idle (no regression to x86)`. Purely additive under `cfg(target_arch="aarch64")`.
- **Implementers per stage:** S0 = athena-architect + athena-kernel; S0.1 = athena-architect; S1–S3 = athena-arch; S4 = athena-arch + athena-kernel; S5 = athena-arch + athena-fs; S6 = athena-arch.
- **MasterChecklist lines to add — Phase 20 (Multi-arch reach):** 20.0 `arch/` boundary (x86 no-regression) · 20.1 xtask `--arch` · 20.2 aarch64 first light (S1) · 20.3 aarch64 MMU+heap (S2) · 20.4 aarch64 VBAR+GIC+timer (S3) · 20.5 aarch64 boot marker (S4) · 20.6 aarch64 install (S5) · 20.7 i686 boot marker · 20.8 aarch64 SMP/desktop (S6, stretch). Also unblocks the deferred MTE line (MTE needs an aarch64 backend first).

### The precise Stage 1 proof

```
# After: xtask build --arch=aarch64 --release   (produces target/aarch64-unknown-none-softfloat/release/kernel)
qemu-system-aarch64 \
  -M virt -cpu cortex-a72 -m 2G -smp 1 -nographic \
  -kernel target/aarch64-unknown-none-softfloat/release/kernel \
  -serial mon:stdio
```

Stage 1 passes iff the serial output contains:

```
[arch:aarch64] PL011 up — AthKernel first light (EL1)
```

…and the guest then idles in `wfi` (no QEMU reset, no repeating exception). Equivalently, the xtask form once S0.1 lands:

```
cargo run -p xtask --release -- run --arch=aarch64 --ci
# → $TEMP/athena-serial.log contains the line above; no "PANIC".
```

---

## Open questions for the lead

1. **`-kernel` first-light vs UEFI from the start?** This plan recommends `-kernel` for S1–S3 (cheapest serial marker) then UEFI for S4+. Confirm — the alternative (UEFI from S1) is more realistic but front-loads the `uefi`-crate entry shim before any ARM code has run.
2. **GICv3 vs GICv2 for S3?** Recommend v3 (the `virt` default, matches real ARM server/laptop). If v3 init stalls during bring-up, is falling back to `gic-version=2` acceptable for the proof, or must S3 be v3?
3. **Phase number:** this doc assigns **Phase 20**. The prior spec said Phase 15 — that slot is taken (AthStore/AthID/AthSync). Confirm Phase 20.
4. **i686 (S/20.7):** still worth building, or defer the "third arch" proof to RISC-V (more strategic for the Rae Station)? (Carried over from the prior spec's open question — unresolved.)
