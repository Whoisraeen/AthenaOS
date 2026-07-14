# M5 — running the compiled amdgpu against the real GPU (the device-backing seam)

**Audience: whoever takes `linuxkpi-drm` from M4 (links) to M5 (runs on Athena).**
Written by the agent that drove the *native* `components/ath_amdgpu` reimplementation
to the MES wall — so this is the device-side + MES knowledge that the header-shim grind
doesn't surface. Cross-refs: `docs/gpu-oracle/` (register traces, firmware RE) and the
`amdgpu-iron-hang-uc-firmware-read` memory.

---

## 0. M4 link target is ready (verified 2026-06-30)

`ath_linuxkpi` builds clean as a C-linkable staticlib — the `#[panic_handler]` is
wired behind the `clib` feature:

```bash
cargo rustc -p ath_linuxkpi --features clib --crate-type staticlib   # → libath_linuxkpi.a
```

So the M4 link object exists today; the remaining M4 work is reconciling the shim struct
layouts (work@24, mutex, timer, dma_fence) against this crate's actual exports, then
linking the amdgpu `.o`s against it.

## 1. The seam: a compiled `.o` is not a running driver

Once `mes_v11_0.o` (+ its IP-block deps) links against `ath_linuxkpi`, the real amdgpu
C still needs **a populated `struct amdgpu_device` and every LinuxKPI call backed by real
device access.** AthenaOS already has all of that — in the **`amdgpud` daemon's `GpuOps`
impl** and `ath_linuxkpi`. Don't rebuild it; *wire to it.* The mapping:

| What amdgpu C calls | AthenaOS backing (already exists) |
|---|---|
| `request_firmware(&fw, name, dev)` | `amdgpud` `request_firmware_bytes()` — loads the blob set in `build_gfx_fw_blobs` |
| `RREG32`/`WREG32` → `amdgpu_device_{r,w}reg` | BAR0 MMIO: `amdgpud` maps BAR0, `reg<<2` byte offset, `ath_linuxkpi::pci::{readl,writel}` |
| `WDOORBELL64` | BAR2 doorbell MMIO: `amdgpud` `doorbell_mmio` + `pci::writeq` |
| `amdgpu_bo_create` + `amdgpu_bo_gpu_offset` | `amdgpud` `dma_alloc` → GART-mapped GPU VA (the `DmaBuf` path) |
| `amdgpu_discovery_reg_base_init` → `adev->reg_offset[ip][inst][seg]` | parse the discovery blob via `ath_amdgpu::discovery` (gc_base=[0x1260,0xa000,0x2402c00,0x2000029,0x10205], mmhub[1]=0x1a000) |
| `pci_read_config_dword` | `amdgpud` `config_read_dword` |
| `request_irq` / IRQ delivery | the capability-gated userspace-driver IRQ-via-IPC path (syscalls 109–118) |
| `ioremap`/`memcpy_toio` | `ath_linuxkpi` ioremap + the VRAM MM_INDEX/MM_DATA path (`vram_write`) |

**Entry point.** Drive amdgpu's `amdgpu_device_init` → per-IP-block `hw_init`, but only
the **MES subset** of IP blocks: `gmc_v11_0` (GART) → `psp_v13_0` (firmware load) →
`gfx_v11_0` (RLC/CP/first light) → `mes_v11_0` → `smu_v13_0` (power) → `ih_v6_0`.
`#if 0` / stub the rest (DC, VCN, JPEG) as SCOPE.md says. The hard prerequisite is
`adev->reg_offset[][]` populated from discovery — without it every `WREG32_SOC15` writes
to garbage.

---

## 2. MES bring-up reality (so you can verify it, not just run it)

When the real `mes_v11_0_kiq_hw_init` / `mes_v11_0_hw_init` runs on Athena:

- **`adev->firmware.load_type` MUST be `AMDGPU_FW_LOAD_PSP`** (Phoenix is secure). Then
  `mes_v11_0_kiq_hw_init` *skips* `mes_v11_0_load_microcode` (the `== DIRECT` branch) and
  the **PSP** loads the MES. The PSP blob set Athena's PSP *accepts* (iron-captured):
  `MES_PIPE0_UCODE=33`, `_DATA=34`, `MES_PIPE1_UCODE=81`, `_DATA=82`. The `RS64_MES`
  types 76–79 are **REJECTED** (0xffff0006) by this PSP — do not use them.
- After `mes_v11_0_enable`, expect (PSP-set, trust them): `IC_BASE=0x4:0x59120000`,
  `DC_BASE=0x4:0x593b0000`, `MDBOUND=0x7ffff`.
- Flow: `mqd_init` → `gfx11_kiq_map_queues` → `amdgpu_ring_test_helper` (the KIQ drain) →
  `set_hw_resources` → `set_hw_resources_1` → `query_sched_status`. (Call tree captured
  in `docs/gpu-oracle/mes_hwinit_graph-20260630.txt`.)

---

## 3. THE point of this whole effort: the `set_hw_resources` halt

**Read this before you celebrate the MES coming up.** The native reimplementation
matched amdgpu **byte-for-byte** — the full warm `wreg` trace (every CP/RLC/cache/int
register), the `set_hw_resources` packet, the PSP-load, IC_BASE/DC_BASE — and the SCHED
pipe (pipe0) **still halts** inside `set_hw_resources` at firmware PC **0x7654**
(`mcause=0`, `sch_ctx=0`, never ACKs). Firmware RE proved the freeze instruction is a
trivial no-op with no memory access → it's a **hardware pipe-clock GATING stall**, not
firmware logic. Clock *frequency* is ruled out (the working driver runs the MES at
800 MHz too). Every register a driver can write, we write. (Full saga:
`amdgpu-iron-hang-uc-firmware-read` memory.)

**So the real test of the `linuxkpi-drm` path is precisely this:** does running the
*complete* real amdgpu init — the full IP-block sequence with the exact timing and
SMU/RLC/PMFW handshakes our hand-port couldn't replicate — get pipe0 **past 0x7654**?

- **If yes** → the broader dynamic init state was the missing piece, and this path
  delivers the MES (and games). That is the entire bet of Strategy B.
- **If it hits the same 0x7654 halt** → the gap is below the driver (SMU/PMFW power
  state, or something the real code *also* doesn't do on AthenaOS's device backing), and
  the next suspect is the **SMU power/handshake sequence**, not more amdgpu code.

**How to verify when you get there** (GRBM-select me=3, pipe=0, read the CP_MES regs):
- `INSTR_PNTR`: working idle = **0x7204**; the halt = **0x7656**.
- `sch_ctx` (the scheduler-context buffer): **non-zero** after a successful
  `set_hw_resources`; **all-zero** = aborted (what we always saw).
- `mcause`/`mepc`/`mbadaddr` (GC seg1 0x281a/0x2818/0x281c): all 0 = no fault.
- Off-target umr on the working driver: `umr --pci 0000:c4:00.0 -RS mes_3.0.0` (un-GFXOFF
  first: `echo 0 | sudo tee /sys/kernel/debug/dri/0/amdgpu_gfxoff`). Athena auto-returns
  to Linux between AthenaOS cold tests, so the live driver is always available as oracle.

---

## 4. Don't relitigate these — already proven dead-ends (native side)

Submission protocol (drain barrier, separate submits), the full register diff (CP int
routing, TCP/TA cache, PA/SPI/GDS engine config — all added), firmware-load (PSP vs
direct), IC_BASE/DC_BASE, GFXCLK frequency. All matched/ruled out. The *only* thing the
native path couldn't replicate is the **dynamic init-sequence/power state** — which is
the one thing running the real code might fix. That's why this track is worth it.
