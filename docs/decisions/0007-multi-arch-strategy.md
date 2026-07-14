# ADR 0007 — Multi-architecture strategy (x86_64 → aarch64 → i686)

- Status: accepted
- Date: 2026-06-21
- Owner: raeen-lead (autonomous); spec by raeen-researcher

## Context
Goal criterion #3 requires AthenaOS to boot, run, and install on x86_64 (current),
aarch64 (ARM 64-bit), and i686 (32-bit x86), each proven independently in QEMU.
raeen-researcher's spec (docs/research/multi-arch-abstraction.md) found: NO `arch::`
boundary exists — the kernel is monolithically x86_64 (385 `x86_64::` refs, 167 inline-asm
sites, `bootloader_api` is x86-only = a hard aarch64 wall). The Concept doc had no explicit
multi-arch clause.

## Decision
1. **Multi-arch is a first-class AthenaOS property** ("anti-walled-garden" — don't lock to one
   silicon vendor). Added a north-star clause to LEGACY_GAMING_CONCEPT.md §Architecture ("Architecture
   Reach") so R10 docstrings have a quote. Reversal: remove that clause.
2. **Introduce a monomorphized `arch::` abstraction** (free-fns/associated-types, NOT `dyn` —
   keep the context-switch hot path indirection-free) over the seams: boot/early-init, CPU init,
   MMU/paging, interrupts, timers, SMP bring-up, context switch, syscall entry, per-CPU base,
   port-vs-MMIO I/O, firmware discovery (ACPI vs DeviceTree). Scheduler/MM/IPC/VFS/AthFS/Rae*
   stay shared. The user/syscall ABI stays arch-neutral (NO `rae_abi`/ABI_VERSION change).
3. **Bring-up order: x86_64 (done) → aarch64 → i686** (charter Step 3.3). Per-arch acceptance =
   the success marker + an `[arch-smoke] <arch> -> PASS` on that arch's QEMU machine
   (aarch64: `qemu-system-aarch64 -M virt`; i686: `qemu-system-i386`).
4. **Sequencing — groundwork now, refactor deliberately:** the heavy Slice 0 kernel refactor
   (relocate ~31 seam files behind `arch::` with ZERO behavior change) is a large, serialized
   change needing ≥5-boot verification; do it as its own focused round + kernel slot, NOT rushed.
   Slice 0a (raeen-architect: `arch/mod.rs` `#[cfg]` re-export skeleton + seam contract +
   `BootContext` replacing `bootloader_api::BootInfo` + R10 smoketest slot) → Slice 0b
   (raeen-kernel: relocate seam files, no logic change) → Slice 1 (arch-neutral MM newtypes,
   the highest-fan-out aarch64 de-risk). No aarch64 code until Slice 1 lands.
5. **aarch64 boot/DTB-firmware needs its own follow-up spec** before coding it (the boot-entry +
   DeviceTree story is large; bootloader_api doesn't apply).

## Rationale
Tie-breaker: (1) the Concept (now) + the /goal make multi-arch authoritative; (2) ISA reach is
a genuine differentiator vs Apple-silicon lock-in; (3) Slice-0-first (zero-behavior-change,
fully QEMU-gated) is the simplest correct path; (4) reversible — the `arch::` boundary is
internal, no ABI change, and Slice 0/1 keep x86_64 booting identically.

## How to reverse
Drop the Concept clause; the `arch::` boundary is a pure internal refactor that can be flattened
back. No external/ABI commitment is made until aarch64 code actually lands.
