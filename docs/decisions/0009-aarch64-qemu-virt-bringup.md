# ADR 0009 — aarch64 (ARM64) QEMU-virt bring-up plan

- Status: accepted (plan); execution gated on Slice 0b (kernel lane)
- Date: 2026-06-22
- Owner: raeen-lead (autonomous)
- Spec: docs/research/aarch64-bringup-spec.md
- Extends: ADR 0007 (multi-arch strategy) — this drills the aarch64 column into a
  QEMU-virt-specific, slice-ordered execution plan (ADR 0007 §5 deferred this).

## Context
Goal criterion #3 requires RaeenOS boot/run/install on x86_64 + aarch64 + i686, each
proven independently in QEMU. x86_64 is live; the arch:: HAL seam (Slice 0, commit
5079a90) landed. aarch64 is the next ISA and the load-bearing proof of the Concept's
"third path … not locked behind Apple silicon" reach clause. The bring-up has several
open hardware-model choices that must be fixed before the A5/A3 implementer starts.

## Decisions
1. **Target the QEMU `virt` machine, cortex-a72, `-smp 4`** as the first aarch64
   platform (deterministic, fully documented memory map from QEMU `hw/arm/virt.c`).
   Real-iron aarch64 is a later, post-QEMU step (iron is paused regardless).
2. **GICv2 first, not GICv3.** Simpler MMIO CPU interface (GICD@0x08000000,
   GICC@0x08010000); GICv3's redistributor/system-register model is deferred to a
   follow-up once interrupts are proven. Reversible: the `arch::Interrupt` seam hides
   the choice; GICv3 can be added behind it.
3. **`-kernel` direct-load, not UEFI/AAVMF**, for CI boot. Deterministic, no OVMF
   variance, matches the verifier's headless model. Entry takes x0=DTB pointer per the
   arm64 Linux boot protocol. UEFI is a later iron-facing option.
4. **DeviceTree (DTB) replaces ACPI on aarch64** — parsed with the `fdt` crate; the
   QEMU-virt DTB is the source of CPU count, RAM base (0x4000_0000), PL011 UART
   (0x0900_0000), and GIC bases. (ACPI stays the x86 path; neither forks the shared
   kernel.)
5. **Slice ordering** (each separately landable + QEMU-verifiable):
   0b (relocate gdt/idt/apic/smp/context/memory behind arch:: traits — x86 stays the
   only impl, must stay 7/7) → 1 (arch-neutral PhysAddr/VirtAddr) → A2 (aarch64 triple
   + xtask --target, compiles) → A3 (boot entry + EL1 + PL011 first byte) → A4 (MMU +
   VBAR_EL1 vectors) → A5 (generic timer + GICv2) → A6 (PSCI SMP) → A7 (boot marker +
   smoketests) → A8 (DTB) → A9 (context switch + svc syscall entry).
6. **Cheapest-proof first:** four pieces are host-KAT-able BEFORE any aarch64 boot —
   the stage-1 page-table descriptor encoder, the GIC register/SGI math, the
   ESR_EL1.EC decode, and the DTB walk (against a `qemu -M virt,dumpdtb` capture).
   These get host KATs ahead of the boot slices to de-risk the riskiest logic
   off-target (the model that root-caused the ACPI 0→159 bug).

## Rationale
Tie-breaker: (1) Concept demands ISA reach; (3) GICv2 + `-kernel` + virt are the
simplest CORRECT options to get a first aarch64 boot now; (4) every choice sits behind
the arch:: seam, so each is reversible (GICv3, UEFI, real iron) without touching shared
kernel logic. Slice 0b is first because it is a pure x86-only refactor — verifiable
IMMEDIATELY as "x86_64 still 7/7," needs no aarch64 code, and unblocks every aarch64
slice.

## Named first hand-off
**Slice 0b** — owner raeen-kernel, with raeen-architect signing the widened arch:: seam
contract (internal kernel ABI; rae_abi ABI_VERSION unchanged). Gated only on the kernel
core lane being free of the concurrent sys_mprotect WIP. Proof line: `[ OS ] System
successfully booted.` + `boot health: 6/6 critical PASS -> HEALTHY`, no [PANIC],
[BOOT-BENCH] not regressed, ≥5 boots SMP=1/=2.

## How to reverse
The plan is docs-only until Slice 0b lands. Each slice is independently revertible;
the arch:: seam keeps GICv2→GICv3, -kernel→UEFI, and virt→iron as later swaps behind a
stable interface. aarch64 code is cfg(target_arch="aarch64") and never compiles into
the x86_64 build, so it cannot regress the live arch.
