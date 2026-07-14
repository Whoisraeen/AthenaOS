# Spec: aarch64 boot + DeviceTree (DTB) bring-up

Status: RESEARCH / follow-up to ADR 0007. Read-only spec — no crate modified by this
document. This is the prerequisite spec ADR 0007 §Decision-5 flagged ("aarch64 boot/DTB-
firmware needs its own follow-up spec before coding it"). It extends
`docs/research/multi-arch-abstraction.md` (the `arch::` seam + Slice 0/1 plan) — read that
first; this spec fills in the single largest unknown in that plan: how an aarch64 kernel
gets control and discovers its hardware **without `bootloader_api` and without ACPI**.

## Concept promise served

> "macOS got locked behind a walled garden of Apple silicon. ... AthenaOS is the third path —
> a from-scratch, embodiment-first, native-feeling OS that treats power users like adults"
> (LEGACY_GAMING_CONCEPT.md §The OS Manifesto)

aarch64 support is the *anti-"locked behind Apple silicon"* property made literal: AthenaOS
runs on ARM64 silicon as a peer architecture, not welded to one ISA. ADR 0007 added a
north-star "Architecture Reach" clause to §Architecture; the aarch64 entry/MMU module's R10
docstring MUST quote one of these lines.

## Already in the tree (verify-before-implement)

Surveyed read-only (a concurrent agent holds uncommitted WIP in `kernel/src`,
`raegfx`, `rae_tokens`, `apps/files` — this spec touched none of it):

- **No aarch64 anything.** Confirms the parent spec: zero `aarch64`/`arm64` tokens in
  `kernel/src`; no `kernel/src/arch/` directory; the `arch::` seam from Slice 0 does not
  exist yet. This spec is pure greenfield design for the aarch64 backend that fills the seam.
- **x86_64 boot entry (the thing aarch64 must replace):**
  `kernel/src/main.rs:305` `use bootloader_api::{... entry_point, BootInfo ...}`;
  `:316` `entry_point!(kernel_main, config = &BOOTLOADER_CONFIG)`;
  `:334` `fn kernel_main(boot_info: &'static mut BootInfo) -> !`. Status `[x]` (iron-proven).
  **`bootloader_api` is x86_64-only — it has no aarch64 backend. This is the hard wall.**
- **What `kernel_main` reads out of `BootInfo` today** (the exact set the arch-neutral
  `BootContext` must carry — grepped from `main.rs`):
  - `boot_info.framebuffer` → `framebuffer::init(fb)` (`:355`, `:542`). Frame buffer fields
    consumed (`framebuffer.rs`): base ptr, byte length, width, height, `stride` (pixels/row),
    `bytes_per_pixel`, `PixelFormat`.
  - `boot_info.physical_memory_offset` (`:420`) → `memory::PHYS_MEM_OFFSET` — the higher-half
    window where all phys RAM is identity-offset-mapped so the kernel can walk page tables.
  - `boot_info.memory_regions` (`:433`, `:436`, `:444`, `:474`, `:479`) → memory-map verify +
    `BootInfoFrameAllocator::init` + `harden_memory_map` + buddy init. Each region: `start`,
    `end`, `kind` (Usable / non-usable).
  - `boot_info.rsdp_addr` (`:496`) → ACPI bring-up (find AP APIC IDs from MADT). **This is the
    field aarch64 replaces with a DTB pointer** — same role ("how do I find CPUs/IRQs"),
    different mechanism.
- **Early serial (the proof-of-life primitive):** `kernel/src/main.rs:352` `serial::init()` is
  the FIRST thing after capturing the TSC; the COM1 16550 UART at `0x3F8` is x86-port-I/O.
  aarch64 QEMU virt has **no port I/O** and **no 16550 at 0x3F8** — it has a PL011 MMIO UART.
  `arch::early_serial` must be the new seam (see §4).
- **MMU on x86_64:** `memory.rs` uses the `x86_64` crate's `OffsetPageTable`/`Cr3`/`PhysFrame`.
  Slice 1 of the parent spec de-`x86_64`-types this behind `arch::mmu::{PhysAddr,VirtAddr,
  Frame,Page,AddressSpace}`. **aarch64 code MUST NOT begin until Slice 1 lands** (ADR 0007 §4).
- `redox_reference/` is **NOT present locally** (verified — glob empty). Harvest from upstream
  Redox read-only when implementing (CLAUDE.md §10.13: never push upstream).

**Delta to design:** the entire aarch64 backend — boot entry asm, BootContext production, a
no_std FDT parser (the ACPI-equivalent firmware seam), MMU bring-up (two TTBRs), PL011 early
serial, GICv2 + generic timer, PSCI SMP — plus the xtask multi-target plumbing to build/run it.

## Prior art & OSS verdict

- **QEMU `-M virt` boot protocol (PRIMARY SOURCE, verified):** with `-kernel`, QEMU loads the
  image and enters it per the **AArch64 Linux boot protocol**: **the DTB pointer is in `x0`**,
  x1–x3 are zero, the image runs at **EL1** by default (EL2 only if `virtualization=on`), MMU
  off, caches off, single core executing (secondaries are parked, released via PSCI). **RAM
  starts at `0x4000_0000`; the DTB is placed at the start of RAM.** Crucially: *"All other
  information about device locations may change between QEMU versions, so guest code must look
  in the DTB."* — this is the whole thesis of §2: **do not hardcode device addresses; parse
  the DTB.** Verdict: in-use convention (QEMU's, not a dependency).
  ([QEMU virt docs](https://www.qemu.org/docs/master/system/arm/virt.html))
- **Redox OS aarch64 port** — the closest from-scratch Rust model. A64 asm stub sets up the
  MMU from scratch, parses the DTB, brings up GICv2 + the ARM generic timer + PSCI for SMP.
  Key harvested insight (already in the parent spec): **two table-base registers
  (`TTBR0_EL1` user / `TTBR1_EL1` kernel)** vs x86's single `Cr3`. Verdict: **📖 study/isolate**
  (MIT, but harvest the *layering + two-TTBR pattern*, never the code; not present locally).
- **`fdt` crate (rust-osdev / `devicetree`)** — pure `no_std` flattened-DeviceTree parser
  (zero-copy over the blob). Verdict: **➕ vendorable** (MIT/Apache). This is the recommended
  base for the firmware seam — see §2. Alternative `fdt-rs` (also permissive). Do **not** clone
  Linux's `drivers/of` — harvest the *FDT binary format* (an open spec), not Linux's OF code
  (CLAUDE.md §4.2 no-clone).
- **The flattened DeviceTree format itself** — the [Devicetree Specification](https://www.devicetree.org/specifications/)
  (v0.4) is the authoritative binary-format + standard-bindings source. This is an open spec,
  not a Linux artifact; parsing it is not a Linux clone.
- **PSCI (Power State Coordination Interface)** — ARM's standard for `CPU_ON`/`CPU_OFF` via
  `SMC`/`HVC` calls. [ARM DEN 0022](https://developer.arm.com/documentation/den0022/latest/).
  The DTB `/psci` node declares the conduit (smc vs hvc) and function IDs.
- **GIC (Generic Interrupt Controller) v2/v3** — [ARM IHI 0048 (GICv2)](https://developer.arm.com/documentation/ihi0048/latest/).
  QEMU virt defaults to GICv2 (override `-machine gic-version=3`). Base addresses come from the
  DTB `interrupt-controller` node — **do not hardcode** (though virt's current GICv2 dist is
  `0x0800_0000`, cpu-if `0x0801_0000`; treat as DTB-derived).
- **`uefi` crate (AAVMF/edk2 path)** — MIT/Apache, works on aarch64; the alternative boot path
  (§1, deferred). Verdict: **➕ vendorable** if we later want a UEFI aarch64 image.

Respecting Concept §R7 (no Linux-clone lineage): we adopt the *AArch64 Linux boot protocol*
(an ABI convention QEMU/firmware implement, not Linux architecture) and the *open FDT format*.
No Linux code or architecture is transplanted.

## Design

### Boot-path decision: `-kernel` raw image FIRST, UEFI later

Two ways onto aarch64:

| Path | Pros | Cons | Verdict |
|---|---|---|---|
| **`-kernel` raw image (AArch64 Linux boot protocol)** | Simplest. No firmware. QEMU hands us `x0=DTB`, EL1, MMU off. Mirrors how we already use `-kernel`-style flow in CI. Fastest proof-of-life. | Image must be a flat binary at the load address with a tiny header; no UEFI services (we don't need them — DTB gives us everything). | **CHOSEN for bring-up.** |
| **UEFI (edk2 AAVMF) + `uefi` crate** | Matches real ARM laptops/servers; gives a GOP framebuffer + ACPI-or-DTB; the eventual "real hardware" path. | Heavier; needs AAVMF firmware blob in xtask; UEFI memory map + ExitBootServices dance. | **Deferred** to a later phase (real-HW story), after QEMU virt boots green. |

**Decision: bring up on `-kernel` + DTB in QEMU virt first; add the UEFI path as a second
boot frontend once the arch backend is proven.** Both frontends converge on the same
`BootContext` (§ below), so the kernel core never learns which one ran.

### The early assembly stub (`arch/aarch64/boot.S` / `naked_asm!`)

QEMU enters at our image's first instruction with: `x0` = DTB phys ptr, EL1 (or EL2), MMU off,
caches off, one core live. The stub's job (minimal, in assembly, before any Rust):

1. **Stash `x0` (DTB pointer)** into a callee-saved reg (e.g. x19) — Rust must receive it.
2. **If entered at EL2** (only when `virtualization=on`): configure `HCR_EL2.RW=1` (EL1 is
   AArch64), set `SPSR_EL2`/`ELR_EL2`, and `eret` down to EL1. Default virt is EL1 — detect via
   `CurrentEL` and skip if already EL1. (Bring-up assumes EL1; handle EL2→EL1 drop defensively.)
3. **Set up a boot stack:** point `sp` at a reserved `.bss`-adjacent boot stack (a static
   16-KiB-aligned array; aarch64 SP must be 16-byte aligned).
4. **Clear `.bss`** (zero from `__bss_start` to `__bss_end` — the linker script provides these).
   x86 got this from the bootloader; on `-kernel` we own it.
5. **Set up exception vectors:** load `VBAR_EL1` with the address of the vector table (16
   entries × 128 bytes; even if most just panic initially, the table must exist before MMU/IRQ).
6. **MMU stays OFF** through the early PL011 print (§4) — MMIO works with MMU off (device memory
   is accessible), so we get proof-of-life *before* paging. Then call into Rust
   `arch::aarch64::rust_entry(dtb_ptr: usize) -> !`.

The stub is the aarch64 analogue of x86's `entry_point!` + the bootloader's setup. It is the
ONLY hand-written aarch64 asm needed for the first milestone; MMU/GIC/timer/PSCI are Rust
poking system registers via `core::arch::asm!`.

### What replaces `bootloader_api::BootInfo` — the shared `BootContext`

This is the seam the parent spec (NEEDS-INTERFACE) names but does not fully shape. Concrete
proposal (owned by `raeen-architect`; arch-neutral, lives in `arch/mod.rs` or a shared
`boot.rs`):

```rust
/// Arch-neutral hand-off from arch::boot to the shared kernel core.
/// Produced by each arch's early entry, consumed by shared kernel_main(ctx).
/// Replaces the direct bootloader_api::BootInfo dependency.
pub struct BootContext {
    /// Usable + reserved physical memory regions (arch fills from DTB /memory
    /// on aarch64, from bootloader_api memory_regions on x86_64).
    pub memory_map: &'static [MemoryRegion],
    /// Optional early framebuffer (None on headless QEMU virt with no ramfb/virtio-gpu yet).
    pub framebuffer: Option<FrameBufferDesc>,
    /// Offset of the all-physical-RAM identity window in the higher half
    /// (x86: bootloader-provided; aarch64: we choose it, e.g. 0xFFFF_8000_0000_0000).
    pub phys_mem_offset: u64,
    /// Firmware-table handle: the ACPI-vs-DTB seam, an explicit enum (no raw ptr ambiguity).
    pub firmware: FirmwareTables,
    /// Kernel command line (installer flag etc.); from DTB /chosen bootargs or bootloader.
    pub cmdline: Option<&'static str>,
}

pub enum FirmwareTables {
    Acpi { rsdp: u64 },              // x86_64 / aarch64-UEFI-with-ACPI
    DeviceTree { dtb: &'static [u8] }, // aarch64 QEMU virt
}

pub struct MemoryRegion { pub start: u64, pub end: u64, pub kind: MemoryKind }
pub enum MemoryKind { Usable, Reserved, Firmware, /* ... */ }

pub struct FrameBufferDesc {
    pub base: u64, pub len: usize,
    pub width: u32, pub height: u32, pub stride_px: u32, pub bytes_per_pixel: u32,
    pub format: PixelFormat,
}
```

`kernel_main` changes (Slice-1-and-later) from `fn kernel_main(&'static mut BootInfo)` to a
shared `fn kernel_main(ctx: BootContext) -> !`. On x86_64, the existing `bootloader_api` entry
stays as a thin shim that builds a `BootContext` from `BootInfo` and calls the shared core —
**zero behavior change on x86_64** (that's exactly the Slice 0/1 contract).

### DeviceTree (DTB) parsing — the ACPI-equivalent firmware seam

aarch64 QEMU virt has **no ACPI** (unless you force `-machine acpi=on` under UEFI). Hardware
topology comes from the **flattened DeviceTree blob** at `x0`. The DTB is a self-describing
binary: a header (magic `0xd00dfeed`, big-endian), a memory-reservation block, a structure
block (a tree of nodes with properties), and a strings block. **All multi-byte values are
big-endian** — the parser must byteswap (a classic first-bug; assert the magic to catch a
bad/byteswapped pointer immediately).

**Approach: vendor the `fdt` crate (`no_std`, zero-copy) behind an `arch::firmware` facade
that produces a shared `PlatformTopology`.** Do NOT scatter raw DTB walks across the kernel —
the rest of the kernel asks `arch::firmware` typed questions, exactly as it would ask an ACPI
layer. The facade extracts:

| What | DTB node / property | Used for |
|---|---|---|
| **RAM regions** | `/memory@*` → `reg` (address/size cells) | `BootContext.memory_map` → frame allocator + buddy |
| **CPU count + IDs (MPIDR)** | `/cpus/cpu@*` → `reg` (= MPIDR affinity), `enable-method` | SMP: how many cores, their PSCI IDs |
| **PSCI conduit + IDs** | `/psci` → `method` (`"smc"`/`"hvc"`), `cpu_on`/`cpu_off` fn IDs | `arch::smp::start_ap` (CPU_ON) |
| **GIC base + version** | `/intc` (`compatible="arm,gic-v2"`/`gic-v3"`) → `reg` (dist, cpu-if / redist) | `arch::irq::init` |
| **ARM generic timer** | `/timer` (`compatible="arm,armv8-timer"`) → `interrupts` (PPIs: secure, non-secure, virt, hyp) | `arch::time` (CNTP) |
| **PL011 UART** | `/pl011@*` (`compatible="arm,pl011"`) → `reg` (base), `/chosen` `stdout-path` | `arch::early_serial` (after MMU; early print uses the well-known base, see §4) |
| **virtio-mmio devices** | `/virtio_mmio@*` → `reg` + `interrupts` | block/net/gpu driver probe (later phase) |
| **cmdline** | `/chosen` → `bootargs` | `BootContext.cmdline` |

**`PlatformTopology` (shared, the ACPI-MADT analogue):**
```rust
pub struct PlatformTopology {
    pub cpus: Vec<CpuDesc>,           // mpidr, enable_method
    pub gic: GicDesc,                 // version, dist_base, cpu_if_or_redist_base
    pub timer: TimerDesc,             // ppi numbers, optional frequency override
    pub uart: Option<UartDesc>,       // base, kind (Pl011)
    pub virtio_mmio: Vec<MmioDevice>, // base, irq
    pub psci: PsciDesc,               // conduit, cpu_on_id, ...
}
```
On x86_64 the same `PlatformTopology` is produced from ACPI/MADT — so the SMP/IRQ/timer code
above the seam is arch-neutral. This is the second large seam ADR 0007 §Rationale and the
parent spec both flagged; this spec gives it a concrete shape.

**Failure modes:** bad magic → panic with the offending pointer (catches byteswap/wrong-x0);
missing required node (`/memory`, `/cpus`, `/intc`, `/timer`) → panic naming the node (a virt
machine always has them; absence means a parse bug); address/size-cell mismatch (the #1 FDT
correctness trap — `#address-cells`/`#size-cells` govern how many u32s each `reg` entry uses
and they vary by node) → the parser must read them per-parent, never assume 2/2.

### MMU bring-up — two TTBRs, 4 KB granule, 4-level

x86_64: one root in `Cr3`, 4-level (PML4→PDPT→PD→PT). aarch64 (4 KB granule, 48-bit VA):
**two roots** — `TTBR0_EL1` for the low half (`0x0000_…`, user) and `TTBR1_EL1` for the high
half (`0xFFFF_…`, kernel) — and 4 levels (L0→L1→L2→L3). The split is selected by VA bit 55 /
`TCR_EL1.T0SZ`/`T1SZ`. This is the parent spec's "widen paging to carry which root" insight.

Bring-up sequence (Rust poking system regs, MMU starts OFF):
1. **MAIR_EL1** — define memory-attribute indices: index 0 = Device-nGnRnE (MMIO), index 1 =
   Normal Write-Back cacheable (RAM). PTEs reference these by `AttrIndx`.
2. **TCR_EL1** — `T0SZ`/`T1SZ`=16 (48-bit VA each half), 4 KB granule (`TG0`/`TG1`),
   inner/outer WB-WA cacheable for table walks, `IPS` = physical-address-size from
   `ID_AA64MMFR0_EL1`.
3. **Build the initial tables:** identity-map the kernel image + DTB + PL011 (so the early
   print survives MMU-on) and create the higher-half phys-RAM offset window
   (`phys_mem_offset`, e.g. `0xFFFF_8000_0000_0000`) so the kernel can reach all RAM — the
   aarch64 analogue of bootloader_api's `physical_memory_offset`. RAM as Normal/index 1,
   PL011/GIC/virtio as Device/index 0.
4. **Load `TTBR0_EL1` (identity) + `TTBR1_EL1` (kernel high-half).**
5. **`isb`; set `SCTLR_EL1.M=1` (+ `C`, `I` for caches); `isb`** — MMU now ON. The next
   instruction fetches through translation; the identity-mapped kernel keeps running.

**`arch::mmu` seam must provide** (from the parent spec, now aarch64-concrete): `AddressSpace`
(wraps "which TTBR root" — a kernel root + per-process user root), `map/unmap/translate`,
`switch_to` (writes `TTBR0_EL1` + a TLB `dsb;isb` / ASID bump — NOT a full flush; use ASIDs to
avoid TLB thrash), `kernel_root` (the `TTBR1_EL1` value). PTE flags differ from x86 (AP[2:1]
for RW/RO + EL0 access, `UXN`/`PXN` for no-execute, `AF` access flag, shareability) — the
arch-neutral `Page`/`Frame`/flags newtypes from Slice 1 map onto these.

### Early serial (PL011 UART) — the first proof-of-life

QEMU virt's PL011 is at **`0x0900_0000`** (the well-known virt base; confirm via DTB
`/pl011` once parsed, but for the *earliest* print we use the constant — it's stable for `-M
virt` and MMIO works with MMU off). Minimal TX (enough to print the success marker):

- The PL011 register block: `UARTDR` (data, +0x00), `UARTFR` (flags, +0x18; bit 5 `TXFF` =
  TX FIFO full, bit 3 `BUSY`). QEMU's PL011 is pre-initialized by the machine, so for bring-up
  we can **skip baud/line config** and just: spin while `TXFF` set, then write the byte to
  `UARTDR`. (A complete init — `UARTIBRD`/`UARTFBRD`/`UARTLCR_H`/`UARTCR` — comes later for real
  HW; QEMU doesn't require it.)
- `arch::early_serial::putc(b)` / `puts(s)` — `volatile` MMIO writes, no locks, no alloc.
  This is what makes `serial_println!` work on aarch64. The shared `serial_println!` macro
  routes to `arch::early_serial` instead of the x86 16550 port path (a `#[cfg]` in the serial
  module, or — cleaner — the serial module calls `arch::early_serial`).

**Milestone (a) prints from this, BEFORE MMU, before DTB parse** — the single most valuable
de-risk: it proves the toolchain, the load address, the entry asm, the stack, and `.bss` clear
all work, with nothing else in the way.

### Security model (arch is mechanism, never policy)

- Every privileged op still routes through `crate::capability` (CLAUDE.md §4.3). The aarch64
  backend is mechanism — `svc #0` syscall entry marshals regs and calls the **shared**
  `syscall::dispatch(nr, args)` (the 3296-line dispatch is arch-neutral and unchanged). The
  entry asm MUST zero caller-controlled scratch registers before handing to shared dispatch,
  exactly as x86's `thread_entry_user` does (parent spec "trap-frame trust").
- The architecture-gate must reject an aarch64 backend that pokes privileged HW (PSCI SMC,
  GIC) without the shared capability check above it.
- Measured boot / secure boot: aarch64 can **stub** attestation initially (parent spec already
  allows this); fTPM-via-DTB or UEFI-secure-boot is a later phase.

## Interface needs (NEEDS-INTERFACE)

For `raeen-architect` (intra-kernel `arch::` seam contract; **no `rae_abi`/ABI_VERSION
change** — the user/syscall ABI is arch-neutral by design; confirm `ABI_VERSION` unchanged):

- **`BootContext` + `FirmwareTables` + `MemoryRegion`/`MemoryKind` + `FrameBufferDesc`** as
  shaped in §Design — the arch-neutral hand-off replacing `bootloader_api::BootInfo` in
  `kernel_main`'s signature. (This is the same `BootContext` the parent spec names; this spec
  finalizes its fields, including the `FirmwareTables` ACPI-vs-DTB enum.)
- **`PlatformTopology`** (CPUs/GIC/timer/UART/virtio/PSCI) — the ACPI-MADT-vs-DTB seam output;
  produced by `arch::firmware::discover(&FirmwareTables) -> PlatformTopology`.
- **`arch::early_serial::{putc, puts}`** — pre-MMU MMIO print contract (x86 maps it to the
  16550 port path; aarch64 to PL011).
- **`arch::mmu::AddressSpace`** must carry "which translation root" (the two-TTBR widening) —
  already named in the parent spec; this spec confirms the aarch64 obligation (`TTBR0`+`TTBR1`,
  ASIDs in `switch_to`).
- **`arch::smp::start_ap(mpidr, entry)`** backed by PSCI `CPU_ON` on aarch64.
- `arch::run_boot_smoketest()` R10 slot (shared, asserts MMU/IRQ/timer/context per arch).

These are the contract `raeen-architect` owns as Slice 0a's internal seam doc. None require an
ABI bump.

## File-by-file plan (aarch64 backend — after Slice 1 lands)

- `kernel/src/arch/aarch64/boot.rs` (+ `naked_asm!`/`global_asm!`): NEW — the A64 early stub
  (stash x0, EL2→EL1 drop, boot stack, `.bss` clear, `VBAR_EL1`, → `rust_entry(dtb)`).
- `kernel/src/arch/aarch64/early_serial.rs`: NEW — PL011 TX at `0x0900_0000`.
- `kernel/src/arch/aarch64/firmware.rs`: NEW — DTB parse via vendored `fdt` →
  `PlatformTopology` + `BootContext` (the ACPI-equivalent seam).
- `kernel/src/arch/aarch64/mmu.rs`: NEW — MAIR/TCR/TTBR0/TTBR1 setup, 4-level tables,
  `AddressSpace` impl (two roots, ASIDs).
- `kernel/src/arch/aarch64/irq.rs`: NEW — `VBAR_EL1` vector table, GICv2 distributor + CPU
  interface init, EOI, `register_handler`.
- `kernel/src/arch/aarch64/time.rs`: NEW — ARM generic timer (`CNTFRQ_EL0`, `CNTP_TVAL_EL0`,
  `CNTP_CTL_EL0`), PPI wiring.
- `kernel/src/arch/aarch64/smp.rs`: NEW — PSCI `CPU_ON` via `smc`/`hvc`, AP rust trampoline.
- `kernel/src/arch/aarch64/context.rs`: NEW — context switch (x19–x30, sp, FP/SIMD if used,
  TTBR0), kernel/user thread trampolines, `svc #0` syscall entry stub → shared dispatch.
- `kernel/src/arch/aarch64/mod.rs`: NEW — re-export + `arch::run_boot_smoketest()` aarch64 arm.
- `kernel/src/arch/aarch64/linker.ld`: NEW — load address (image at start of RAM region used by
  `-kernel`), `__bss_start`/`__bss_end`, boot-stack reservation.
- `kernel/src/serial.rs`: route the early-print path through `arch::early_serial` (cfg seam).
- `components/vendored/fdt/` (or `Cargo.toml` dep): vendor the `fdt` crate (MIT/Apache).
- `xtask/src/main.rs`: `--target aarch64-unknown-none-softfloat` plumbing; build the aarch64
  flat image; `qemu-system-aarch64 -M virt -cpu cortex-a72 -smp 4 -kernel <img>` run path; the
  CI marker drain (reuse the x86 `[ OS ] System successfully booted.` wait). Today all qemu/
  target strings are hardcoded x86_64 (`xtask/src/main.rs:281,323,1324,...`).
- `rust-toolchain.toml`: add `aarch64-unknown-none-softfloat` to `targets`.
- `.cargo/config.toml`: per-target `rustflags` block for the aarch64 triple.

## Acceptance criteria — staged milestones (smallest-increment-first)

Each milestone is independently QEMU-verifiable; do them in order, commit between. The exact
boot-log line that PROVES each (and a smoketest that can print FAIL — CLAUDE.md §16):

| # | Milestone | Exact proof line(s) | What it de-risks |
|---|---|---|---|
| **a** | **Early PL011 print** (stub: stack + `.bss` + PL011, MMU OFF) | `[arch] aarch64 hello from EL1 (pre-MMU)` printed via PL011 at `0x0900_0000` | toolchain, load addr, entry asm, stack, `.bss` clear, serial — the single biggest unknown, proven first |
| **b** | **MMU on + Rust `kernel_main` reached** | `[mmu] aarch64 4KB-granule 4-level: TTBR0/TTBR1_EL1 active` then `[arch] aarch64 cortex-a72 EL1 -> reached kernel_main` | MAIR/TCR/table build/SCTLR.M — kernel survives translation-on |
| **c** | **DTB parsed** | `[fdt] DTB magic OK @ {ptr}` then `[fdt] N memory regions, GICv2 @ {dist}, timer PPI {n}, PL011 @ {base}, M cpus` | the ACPI-equivalent firmware seam; address/size-cell correctness |
| **d** | **GIC + generic timer interrupts** | `[gic] GICv2 dist+cpu-if online` then `[timer] CNTP armed {Hz} Hz -> tick` (a real timer IRQ taken and EOI'd) | `VBAR_EL1` vectors, GIC enable, PPI delivery, EOI |
| **e** | **Success marker + arch smoketest** | the SAME `[ OS ] System successfully booted.` then `[arch-smoke] aarch64 -> PASS` (asserts: MMU round-trips a known phys↔virt map; a self/SGI IPI is delivered+acked; monotonic clock advances across a busy-wait; a kernel-thread context switch preserves x19–x30) | end-to-end single-core boot to userspace-ready |
| **f** | **PSCI SMP** | `[smp] PSCI CPU_ON: 4/4 cores online` (each AP prints `[smp] cpu{mpidr} alive`) | secondary bring-up, per-CPU `TPIDR_EL1`, the scheduler on >1 aarch64 core |

`/proc/raeen/arch` MUST report (arch-neutral, both arches): active arch (`aarch64`), CPUs
online, page-table levels (4) + root count (2 TTBRs), IRQ controller (`GICv2`), timer
(`arm-generic`), syscall-entry mechanism (`svc #0`), firmware source (`DeviceTree`).

The aarch64 entry/MMU module's R10 docstring MUST quote the Manifesto/Architecture-Reach line
above. SMP milestone (f) needs ≥5 boots at `-smp 1` and `-smp 4` (CLAUDE.md §17 applies per
arch).

## Toolchain + QEMU invocation (concrete)

- **Target triple:** `aarch64-unknown-none-softfloat` (softfloat to match the kernel's
  no-SIMD-in-kernel posture, mirroring `x86_64-unknown-none`'s soft-float; userspace can enable
  FP/SIMD via `CPACR_EL1` like x86 enables SSE via CR4).
- **Build:** `cargo build -p kernel --target aarch64-unknown-none-softfloat` (+ `-Z build-std`
  if the target needs core/compiler-builtins rebuilt — likely yes for a bare `-none` target).
  Produce a flat image for `-kernel` (objcopy-to-binary or a properly-headed ELF QEMU accepts).
- **Run (the milestone command):**
  ```
  qemu-system-aarch64 -M virt -cpu cortex-a72 -smp 4 -m 2G \
    -kernel target/aarch64-unknown-none-softfloat/release/kernel.img \
    -serial mon:stdio -display none -no-reboot
  ```
  PL011 → stdio (the serial log, same role as x86's COM1). `-display none` for headless CI
  (no framebuffer yet → `BootContext.framebuffer = None`; add `-device ramfb` or
  `-device virtio-gpu-pci` when the GFX phase reaches aarch64).
- **xtask:** add `--target`/arch selection that picks `qemu-system-aarch64` + the `-M virt`
  arg set; reuse the existing CI marker-wait (`[ OS ] System successfully booted.`) and
  `%TEMP%\raeen-serial.log` drain. No iron path for aarch64 yet — QEMU virt is the proof
  surface (`[~]` status until/unless real ARM hardware is acquired).

## Risks & realistic phase breakdown (honest)

This is **multi-phase and multi-week**, gated behind Slice 0/1 (the parent-spec x86_64
refactor). Phases, smallest-first:

- **Phase A0 (blocking precondition):** Slice 0a/0b/1 from the parent spec land — the `arch::`
  boundary exists and the MM is de-`x86_64`-typed. **No aarch64 code before this** (ADR 0007
  §4). Risk: low (mechanical, fully QEMU-gated on x86_64), but it's a serialized large change.
- **Phase A1 — proof-of-life (milestone a):** entry asm + PL011. *Highest value, lowest risk.*
  1 focused round. The unknowns (load address, image format QEMU accepts, `-Z build-std` for
  the target) all surface here cheaply.
- **Phase A2 — MMU + DTB (milestones b, c):** the two-TTBR MMU + the FDT parser facade. Medium
  risk (MAIR/TCR mistakes are silent; FDT big-endian + address/size-cells are classic bugs).
  The FDT parser is **host-KAT-able** against a saved QEMU DTB dump (`qemu … -machine
  dumpdtb=virt.dtb`) — do that FIRST on the dev box, like the AML parser was proven off-target.
- **Phase A3 — IRQ + timer (milestone d):** GICv2 + generic timer + `VBAR_EL1`. Medium risk
  (vector-table layout, GIC enable/EOI ordering).
- **Phase A4 — boot to marker + smoketest (milestone e):** wire the shared kernel core over the
  aarch64 seam to the success marker. Risk depends on how many in-kernel paths assumed x86
  (the parent spec's "drivers assume MMIO+x86 IRQ" caveat — but the *core* boot to marker needs
  no NIC/NVMe).
- **Phase A5 — PSCI SMP (milestone f):** `CPU_ON`, per-CPU base, scheduler on N cores. Medium
  risk; reuse the x86 SMP-verification discipline (≥5 boots, smp1+smp4).
- **Later phases (out of scope here):** virtio-mmio block/net/gpu drivers on aarch64; the UEFI
  boot frontend for real ARM hardware; installer on aarch64; FP/SIMD-in-userspace; measured
  boot. Each is its own slice once the core boots.

**Known traps (call out before coding):**
1. **DTB is big-endian** — every multi-byte read byteswaps. Assert `0xd00dfeed` first.
2. **`#address-cells`/`#size-cells` vary per node** — never assume 2/2 when decoding `reg`.
3. **EL2-vs-EL1 entry** — default virt is EL1, but `virtualization=on` enters EL2; the stub
   must detect `CurrentEL` and drop if needed.
4. **MMU-on must keep the early code mapped** — identity-map the kernel image + PL011 + DTB
   before flipping `SCTLR_EL1.M`, or the next fetch faults into the void (no serial to debug).
5. **Don't hardcode device bases beyond the earliest PL011** — QEMU explicitly warns device
   locations may change; everything after milestone (a) comes from the DTB.
6. **`-Z build-std`** is almost certainly required for `aarch64-unknown-none-softfloat`
   (no precompiled core for the softfloat variant) — surfaces in Phase A1.
7. **Don't let the seam go `dyn`** (parent spec hot-path tax) — monomorphized free-fns; the
   `[BOOT-BENCH]` gate guards x86, and aarch64 inherits the same discipline.

## Handoff

- **raeen-architect:** finalize the `BootContext` / `FirmwareTables` / `PlatformTopology` /
  `arch::early_serial` / `arch::mmu::AddressSpace`(two-root) / `arch::smp::start_ap` seam
  signatures from §Interface (Slice 0a internal contract; confirm `ABI_VERSION` unchanged). This
  spec gives the concrete field shapes; architect ratifies them.
- **raeen-kernel / raeen-arch:** implement the aarch64 backend in milestone order
  (a→f) — entry asm + PL011 → MMU(two-TTBR) → DTB/FDT parser facade → GICv2 + generic timer →
  boot-to-marker + smoketest → PSCI SMP. Plus xtask multi-target + the host-KAT FDT parser
  proven against a `dumpdtb` capture before QEMU.
- **Unblocks:** the aarch64 workstream of owner goal criterion #3 ("boot, run, install on …
  aarch64"), once Slice 1 (arch-neutral MM types) lands. No MasterChecklist line exists yet —
  recommend the lead add the "Multi-architecture support" phase (proposed in the parent spec)
  with milestones a–f above as its aarch64 sub-items.
- **Sequencing:** strictly after Slice 1 (parent spec / ADR 0007 §4). Then A1→A5 in order; each
  milestone is its own commit with its proof line. Host-KAT the FDT parser first.

---
Sources:
- [QEMU `virt` machine docs — boot protocol, x0=DTB, RAM@0x4000_0000](https://www.qemu.org/docs/master/system/arm/virt.html)
- [Devicetree Specification v0.4 (FDT binary format + standard bindings)](https://www.devicetree.org/specifications/)
- [ARM PSCI (DEN 0022)](https://developer.arm.com/documentation/den0022/latest/)
- [ARM GICv2 architecture spec (IHI 0048)](https://developer.arm.com/documentation/ihi0048/latest/)
- [ARM Architecture Reference Manual (ARMv8-A) — MAIR/TCR/TTBR/SCTLR/VBAR_EL1, generic timer](https://developer.arm.com/documentation/ddi0487/latest/)
- [QEMU AArch64 Virt Bare Bones (OSDev)](https://wiki.osdev.org/QEMU_AArch64_Virt_Bare_Bones)
- [PL011 UART Technical Reference Manual](https://developer.arm.com/documentation/ddi0183/latest/)
- [`fdt` crate (no_std FDT parser, MIT/Apache)](https://crates.io/crates/fdt)
- [Redox aarch64 port (study-only; harvest two-TTBR + layering pattern)](https://gitlab.redox-os.org/redox-os/kernel)
- parent spec: docs/research/multi-arch-abstraction.md · ADR: docs/decisions/0007-multi-arch-strategy.md
