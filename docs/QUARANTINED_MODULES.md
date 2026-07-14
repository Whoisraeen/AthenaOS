# Quarantined / removed kernel scaffolds

These files were **duplicate orphans** (not `pub mod` in `main.rs`) with `TODO` stubs that duplicated live modules. They were removed to avoid two sources of truth.

| Removed file | Use instead |
|--------------|-------------|
| `kernel/src/edid_parse.rs` | `kernel/src/edid.rs` |
| `kernel/src/iommu_enforce.rs` | `kernel/src/iommu.rs` (`sandbox_device_dma`) |
| `kernel/src/tpm_2_0.rs` | `kernel/src/tpm.rs` |
| `kernel/src/hybrid_topology.rs` | `kernel/src/smp.rs` (`detect_local_topology`) |
| `kernel/src/sleep_states.rs` | `kernel/src/suspend.rs` |
| `kernel/src/ec.rs` | `kernel/src/acpi_full.rs` |
| `kernel/src/x2apic_msr.rs` | `kernel/src/apic.rs` |
| `kernel/src/fat32.rs` | MasterChecklist Phase 3 installer / future AthFS bridge |
| `kernel/src/gpt.rs` | `kernel/src/block_io.rs` + `storage_mount.rs` |
| `kernel/src/usb_hid.rs` | `kernel/src/xhci.rs` (the LIVE driver — `usb/xhci.rs` is scaffold), `input.rs`, `usb_core.rs` |
| `components/raegfx/src/font.rs` (orphan — never `pub mod` in `raegfx/lib.rs`; a 2nd TrueType `FontEngine` twin) | `components/raefont` (the WIRED engine — hinting interpreter, COLR/CPAL, shaper, the crisp filled rasterizer + `builtin` faces). `Canvas::draw_text_aa` (raegfx `text.rs`) is the one text API. (typography-rendering.md §3.2) |

> Note: `kernel/src/usb_msc.rs` was later re-created as a real, wired module (init + smoketest in `main.rs`) — it is live, not quarantined.

## Removed in-file dead twins (rule 7)

These were dead blocks **inside an otherwise-live file** — non-functional structural twins of a live driver. The block (and any helper types it exclusively owned) was deleted; the live module is the single source of truth.

| Removed block | Live module to use instead | Date | Why |
|---------------|----------------------------|------|-----|
| `net_drivers::VirtioNetDriver` (+ exclusively-owned `VirtQueue`/`VirtQueueDesc`/`VirtQueueAvail`/`VirtQueueUsed(Elem)`/`VirtioNetHeader`/`VirtioNetFeatures` + `VIRTIO_NET_F_*`/`VRING_DESC_F_*`/`VIRTIO_NET_HDR_SIZE` consts, in `kernel/src/net_drivers.rs`) | `kernel/src/virtio_net.rs::VirtioNet` (wired via `virtio_net::init(dev)`) | 2026-06-17 | Never constructed (`VirtioNetDriver::new` had zero callers); non-functional anyway — software `VirtQueue` ring whose descriptor `addr` was a **virtual** `buf.as_ptr()` never programmed into a real device queue. Bought zero capability. The live `VirtioNet` uses a persistent pre-allocated TX buffer with real DMA. |

New work for those features must extend the **wired** module and update `MasterChecklist.md`.
