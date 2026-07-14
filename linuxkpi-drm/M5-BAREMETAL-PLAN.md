# M5 — running the real amdgpu on bare-metal Athena (the goal)

**Goal:** fully port the Linux amdgpu driver to bare metal so RaeenOS gets 3D
graphics + gaming acceleration. The compile/link work is done; this is the run.

**Status (2026-06-30):** the real upstream amdgpu Phoenix bring-up graph **compiles,
links, and is init-crash-safe** — `bash linuxkpi-drm/m4c-link.sh`: 74 real objects
(GFX/GMC/IH/PSP/SDMA/MES/SOC21 + drm_sched + TTM + SMU/dpm + the DRM core), 0
unresolved against raeen_linuxkpi, **on-path stubs = 0** (every symbol a real
`amdgpu_device_init` executes is real or a valid no-op descriptor / real module
param; the 446 remaining stubs are all off-path — other ASICs, SR-IOV, KFD, video,
swap, mmap — never reached on a headless Phoenix GFX init).

---

## Step 0 — cross-build viability: ✅ VERIFIED

The amdgpu source compiles **freestanding** (bare-metal, no host libc):
```
gcc -std=gnu11 -ffreestanding -fno-stack-protector -fno-strict-aliasing -m64 \
    -mno-red-zone -c amdgpu/amdgpu_device.c $INC -> amdgpu_device.o (0 errors)
```
(verified for amdgpu_device / gfx_v11_0 / mes_v11_0 — the most complex TUs). So the
whole object set can be built for the RaeenOS daemon target with the same shim.

---

## The remaining path to a live run

### 1. Cross-build the object set + raeen_linuxkpi for RaeenOS
- Compile all `BRINGUP*` .c with `-ffreestanding -mno-red-zone` for x86_64.
- Build raeen_linuxkpi for `x86_64-unknown-none` (already the daemon/kernel target;
  the `clib` staticlib is currently built for `-linux-gnu` for the host link test —
  add a `-none` build).
- `ld -r` them into `amdgpu-bringup-baremetal.o`, link into the `amdgpud` daemon.

### 2. Wire the device-access facade to the LIVE GPU (the key gap)
Today raeen_linuxkpi's MMIO/PCI accessors split:
- `readl/writel/pci_read_config_dword` (lib.rs) → real, route to the RaeenOS device
  syscalls (`host::sys_*`). ✅
- `ioremap`/`pci_iomap`/`dma_alloc_coherent` in `drm_bringup.rs` → **null/no-op link
  stubs** (so the host m4c link resolves without a device). ❌ for a live run.

For the live daemon build these must map the REAL Athena BARs via the device-claim
the daemon already uses for its `GpuOps` impl (BAR5=regs, BAR2=doorbell,
`sys_claim_device` → `sys_ioremap(handle, bar)`). Options:
- a `live` cargo feature on raeen_linuxkpi that swaps the drm_bringup device stubs
  for `pci::ioremap`/`host::sys_ioremap`-backed real maps, OR
- move those exports out of drm_bringup into the real `pci`/`dma` modules for the
  daemon build and keep the null stubs only under the host-link-test feature.

The amdgpu C then drives the GPU through the SAME device path as the Rust reimpl's
`GpuOps` (`config_read_dword`/`map_register_bar`/`reg_read`/`reg_write`/`dma_alloc`/
`ring_doorbell`), because those map 1:1 onto raeen_linuxkpi's C-ABI accessors.

### 3. A daemon entry that calls `amdgpu_device_init`
- Claim the GPU (BDF `c4:00.0` on Athena), allocate a `struct amdgpu_device`,
  populate `adev->pdev`/BAR info from the claim, then call the real
  `amdgpu_pci_probe` → `amdgpu_driver_load_kms` → `amdgpu_device_init` (or call
  `amdgpu_device_init` directly with a minimal adev, skipping the modprobe glue).
- `jiffies` must tick (raeen_linuxkpi `lkpi_tick_jiffies`) so amdgpu's timeout loops
  terminate; the workqueue/timer pump must be driven each daemon loop.

### 4. Run on Athena → the 0x7654 test
Flash (safe image) or the KVM/VFIO no-flash loop, run the daemon, and watch whether
the COMPLETE real init clears the `0x7654` pipe0 microengine halt that our
byte-identical Rust reimpl could not (see the amdgpu-iron memory). If it does, the
MES is alive → GFX ring submit → the first real `vkQueueSubmit`-equivalent → 3D.

### 5. Then: GFX submit → scanout → gaming
GART/GTT + a GFX ring submit (already real in the object set) → direct scanout on
the panel (modeset, EDID) → RaeGFX's Vulkan-equivalent path (Mesa RADV lineage over
this driver) → games via RaeBridge/Proton.

---

## Why this is the right architecture
The Rust reimpl (`raeen_amdgpu/src/bringup.rs`, ~6.5k lines over `GpuOps`) matches
amdgpu byte-for-byte yet halts at 0x7654 — so the gap is in the *broader* init the
complete driver does (power/clock/SMU/RLC/IH state that keeps the microengine
alive). Running the REAL, complete `amdgpu_device_init` — all 74 objects, real
TTM/SMU/drm_sched — is the way to establish that broader state. That is now a
build+wire task, not a reverse-engineering one.
