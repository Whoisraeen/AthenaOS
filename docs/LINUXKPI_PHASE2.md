# LinuxKPI Phase 2-4 — the hardware bridge

**Status:** QEMU host smoketest PASS (`[~]`). Real-driver port (amdgpu/iwlwifi) + Athena pending.

## Goal

Run **unmodified Linux driver source** (`amdgpu`, `iwlwifi`, `e1000e`, …) as an
IOMMU-sandboxed userspace daemon that *believes* it is in the Linux kernel with
Ring-0 privileges. Every privileged call is intercepted by the LinuxKPI host and
translated into native AthKernel primitives. This delivers Day-1 hardware breadth
without rewriting millions of lines of C — per Concept §Architecture
("drivers IOMMU-sandboxed") and `docs/LINUX_DRIVER_STRATEGY.md` Path C/D.

## The four phases

### Phase 1 — Foundation (the great deception)
`components/ath_linuxkpi` exposes the C-ABI the Linux kernel exports:
- `kmalloc`/`kzalloc`/`kfree` → daemon-local bump heap (`mm.rs`)
- `get_jiffies_64`/`msleep` → host syscalls 128/129 → `timers::JIFFIES` + HPET sleep
- `athena_printk` → host syscall 131 → kernel serial
- `spin_lock`/`spin_unlock`, `mutex_lock`/`mutex_unlock` → atomics + scheduler yield

### Phase 2 — Hardware bridge (MMIO + interrupts)
- **`pci_enable_device(bus,dev,func)`** (syscall 132) → `userspace_driver::register` +
  `sys_claim_device` (109–111) mints `Cap::Mmio` + `Cap::Irq` in the daemon task,
  plus `iommu::create_device_domain` and bus-master enable. Returns an opaque
  LinuxKPI device handle.
- **`ioremap(dev, bar)`** (syscall 130) → probes BAR size, maps into the **daemon
  user PML4** at `0x5000_0000 + bar*0x10_0000` via `memory::map_phys_mmio_into_current_task`
  (not the kernel `phys_to_virt` window). `readl`/`writel` are plain volatile ops.
- **`pci_read/write_config_dword`** (syscalls 133/134) → ownership-gated config access.
- **`request_irq(dev, vector)`** (syscall 137) → returns the `Cap::Irq` handle minted
  at claim time (MSI index 0..n-1 → hardware IDT vector in the cap). `irq_wait`
  (syscall 138) blocks like syscall 8 (`BlockedOnIrq`); pending IRQs are not lost
  if delivery happens before `irq_wait`. `interrupts::dispatch_msi` and
  `lkpi_deliver_irq(vector)` call `scheduler::unblock_irq_waiters`. Supervisor op 4
  (`SUP_TRIGGER_DEV_IRQ`) injects a doorbell for QEMU smoketest.

### Phase 3 — Zero-copy data path (preserve sub-frame latency)
- **`dma_alloc_coherent(dev, size, &dma_handle)`** (syscall 135) → allocates
  physically-contiguous frames (`memory::allocate_contiguous_frames`), programs the
  device IOMMU domain to permit DMA into **exactly** those frames
  (`iommu::sandbox_device_dma`), returns `[virt, phys, size, token]`.
- The driver writes only **metadata** (ring head/tail, descriptor addresses) using
  `virt`, and programs `phys` (the DMA address) into the hardware. The actual
  payload (textures, vertices, packets) is written by the **app** directly into the
  same physical frames via a shared-memory capability. The LinuxKPI host copies
  **zero bytes** — sub-frame latency is preserved.

### Phase 4 — Sandboxing + containment
- **IOMMU enforcement:** each device lives in its own domain; it can only DMA into
  frames granted via `dma_alloc_coherent`. A buggy C driver pointing the hardware at
  kernel memory is blocked at the silicon (`note_iommu_block` counts it).
- **Daemon restarts:** `lkpi_supervisor_register` enrolls the daemon (syscall 140).
  On fault, `supervisor_on_fault(dev_handle)` clears bus-mastering (so the dead
  driver's device can't DMA mid-restart), tears down DMA regions + IRQ channels, and
  increments the restart count so a watchdog relaunches the daemon ELF. Screen
  flickers; the OS stays alive.

## Layout

| Path | Role |
|------|------|
| `kernel/src/linuxkpi_host.rs` | Host syscalls 127-140 + device registry + supervisor |
| `components/ath_linuxkpi/src/host.rs` | Raw syscall stubs (syscall1/2/3 helpers) |
| `components/ath_linuxkpi/src/pci.rs` | `pci_enable`, `ioremap`, `readl`/`writel` |
| `components/ath_linuxkpi/src/pci_ext.rs` | `pci_set_master`, regions, config byte/word, `pci_resource_*`, `pci_alloc_irq_vectors`, `pci_find_capability`, `dma_set_mask_*` |
| `components/ath_linuxkpi/src/dma.rs` | `dma_alloc_coherent` zero-copy bridge |
| `components/ath_linuxkpi/src/dma_stream.rs` | `dma_map_single`/`dma_map_page`/sync/`dma_mapping_error` (streaming) |
| `components/ath_linuxkpi/src/irq.rs` | `request_irq`/`request_threaded_irq`/`free_irq`/`enable_irq`, handler registry + `lkpi_serve_irq` pump |
| `components/ath_linuxkpi/src/string.rs` | `memcpy`/`memmove`/`memset`/`memcmp`/`memchr` + `str*` |
| `components/ath_linuxkpi/src/atomic.rs` | `atomic_t`/`atomic64_t` ops, `*_bit`, `mb`/`rmb`/`wmb` barriers |
| `components/ath_linuxkpi/src/delay.rs` | `udelay`/`ndelay`/`mdelay`/`usleep_range`, `ktime_get_ns`, jiffies conv |
| `components/ath_linuxkpi/src/kalloc.rs` | `vmalloc`/`kvmalloc`/`kcalloc`/`krealloc`/`kmemdup`/`kstrdup`/`devm_*`/page alloc |
| `components/ath_linuxkpi/src/sync.rs` | spinlocks (`_raw_spin_*`, irqsave), mutex init/trylock, `struct completion`, rwsem, `schedule`/`cond_resched` |
| `components/ath_linuxkpi/src/device.rs` | `printk`/`_printk`, `dev_err`/`dev_warn`/`dev_info`/`dev_dbg`, `dump_stack` |
| `components/ath_linuxkpi/src/lib.rs` | `kmalloc`/`kfree`, firmware, supervisor, self-tests |

## Symbol surface (2026-06-10)

The shim went from ~31 to **168 exported C-ABI symbols** — the universal
primitive families every Linux driver references (a `.ko` fails to link if any
undefined symbol is unresolved). Categories now covered: memory (kmalloc family
+ vmalloc + devm + pages), string/mem builtins, atomics + bitops + barriers,
delays + monotonic time, locking + completions, PCI (enable/master/regions/
config all widths/resource sizing/cap walk/IRQ vectors), IRQ register+dispatch,
coherent + streaming DMA, firmware load, device logging.

**What still gaps a *full* real driver** (honest scope): a C vararg formatter so
`printk("%d")` interpolates (today the format string prints verbatim); a host
`DMA_PIN(va,len)->iova` syscall for correct streaming DMA on arbitrary buffers;
`struct page`/scatterlist, workqueue/timer (`schedule_delayed_work`,
`mod_timer`), and the per-subsystem facades (`netdev`/`drm`/`cfg80211`). These
are the next tranche; the current surface is enough to link + run early
`*_probe()` init paths.

## Boot proof (QEMU)

```
[linuxkpi] host ready: syscalls 127-140 …
[linuxkpi] host smoketest: … bridge=usdriver+caps -> PASS
[linuxkpi] pci_enable … -> lkpi=N claim=0x… mmio_cap=… irqs=…
[linuxkpi] ioremap dev=N BAR0 phys=… -> user_virt=0x50000000
[user-thread] msg: 8000
[user-thread] msg: 8104    # self_test_phase2 (pci+ioremap+irq cap+irq_wait)
[user-thread] msg: 8200    # irq_wait delivered
[linuxkpi] supervisor: trigger_dev_irq dev=N vector=…
[linuxkpi] host smoketest: … irq_delivery=dispatch_msi+pending …
[user-thread] msg: 7900
```

```powershell
Select-String -Path target\serial-input.log -Pattern "bridge=usdriver","msg: 810","ioremap.*user_virt"
```

## What this unblocks

- **MasterChecklist Phase 2.5:** AMD GPU (Radeon 780M) / Intel GPU via userspace Mesa.
- **MasterChecklist Phase 2.2:** Wi-Fi via userspace `iwlwifi` daemon.
- **MasterChecklist Phase 6.1:** AMDGPU/i915 DRM-equivalent driver hosted in userspace.

## Not yet done

- A real driver port (amdgpu is ~3M lines C; first target is a minimal modeset path).
- MSI-X table programming + `lkpi_deliver_irq` wired from device drivers (not only
  synthetic tests).
- IOMMU fault → `note_iommu_block` wiring on real VT-d/AMD-Vi hardware.
- Shared-memory capability (`Cap::SharedMemory`) for the app→hardware zero-copy path;
  today DMA frames are kernel-mapped and the daemon reads `phys` directly.
