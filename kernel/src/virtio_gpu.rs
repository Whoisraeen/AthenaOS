//! virtio-gpu driver — the QEMU on-ramp for CPU→GPU rendering (Phase 6).
//!
//! Concept §AthGFX / "the GPU draws the pixels": today the compositor software-
//! rasterizes into the UEFI GOP framebuffer and the CPU copies every pixel. The
//! transition to GPU rendering needs a real device that owns a framebuffer
//! *resource* and *scans it out* itself. Under QEMU the GPU is `virtio-gpu`, so
//! this driver is the first concrete step of that transition: it brings the
//! virtio-gpu device up over the legacy virtio-pci transport, drives its control
//! queue, and exercises the exact command sequence real GPU presentation uses —
//!
//!   GET_DISPLAY_INFO → RESOURCE_CREATE_2D → RESOURCE_ATTACH_BACKING →
//!   SET_SCANOUT → TRANSFER_TO_HOST_2D → RESOURCE_FLUSH
//!
//! That is the device-side equivalent of "allocate VRAM, point the scanout at
//! it, and flip" — i.e. hardware page-flipping with the CPU out of the copy
//! path. Real `amdgpu`/`i915` + Mesa/Vulkan layer on top of this same model.
//!
//! We attach virtio-gpu as a *secondary* adapter so the bootloader's GOP (from
//! the primary stdvga) is never disturbed; this driver proves the GPU command
//! path independently before the compositor is repointed at it.
//!
//! R10: `init()` + `run_boot_smoketest()` + `/proc/raeen/virtio_gpu`
//! (`dump_text`) + this Concept docstring.

#![allow(dead_code)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

// ── virtio-gpu control commands ─────────────────────────────────────────────
const CMD_GET_DISPLAY_INFO: u32 = 0x0100;
const CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
const CMD_SET_SCANOUT: u32 = 0x0103;
const CMD_RESOURCE_FLUSH: u32 = 0x0104;
const CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
const CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;
// Cursor queue commands (queue 1). QEMU consumes these off the used ring but
// writes NO response header (virtqueue_push len=0), so success is detected by
// used-ring advance, not a RESP_* type — see `submit`'s u32::MAX timeout
// sentinel and `submit_consumed`.
const CMD_UPDATE_CURSOR: u32 = 0x0300;
const CMD_MOVE_CURSOR: u32 = 0x0301;
// Responses
const RESP_OK_NODATA: u32 = 0x1100;
const RESP_OK_DISPLAY_INFO: u32 = 0x1101;
// Pixel format: BGRA8888 (matches our framebuffer convention).
const FORMAT_B8G8R8A8_UNORM: u32 = 1;
const MAX_SCANOUTS: usize = 16;

// ── virtio-pci descriptor flags ─────────────────────────────────────────────
const VRING_DESC_F_NEXT: u16 = 1;
const VRING_DESC_F_WRITE: u16 = 2;

// ── modern virtio-pci common-configuration field offsets ────────────────────
const CC_DEVICE_FEATURE_SELECT: u64 = 0;
const CC_DEVICE_FEATURE: u64 = 4;
const CC_DRIVER_FEATURE_SELECT: u64 = 8;
const CC_DRIVER_FEATURE: u64 = 12;
const CC_NUM_QUEUES: u64 = 18;
const CC_DEVICE_STATUS: u64 = 20;
const CC_QUEUE_SELECT: u64 = 22;
const CC_QUEUE_SIZE: u64 = 24;
const CC_QUEUE_ENABLE: u64 = 28;
const CC_QUEUE_NOTIFY_OFF: u64 = 30;
const CC_QUEUE_DESC: u64 = 32;
const CC_QUEUE_DRIVER: u64 = 40;
const CC_QUEUE_DEVICE: u64 = 48;

// device_status bits
const S_ACK: u8 = 1;
const S_DRIVER: u8 = 2;
const S_DRIVER_OK: u8 = 4;
const S_FEATURES_OK: u8 = 8;
const S_FAILED: u8 = 128;

// virtio cap cfg_type
const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;
const VIRTIO_PCI_CAP_NOTIFY_CFG: u8 = 2;
const VIRTIO_PCI_CAP_DEVICE_CFG: u8 = 4;

// VIRTIO_F_VERSION_1 is feature bit 32 (bit 0 of the high feature dword).
const VIRTIO_F_VERSION_1_HI: u32 = 1;

#[inline]
unsafe fn mr8(a: u64) -> u8 {
    core::ptr::read_volatile(a as *const u8)
}
#[inline]
unsafe fn mr16(a: u64) -> u16 {
    core::ptr::read_volatile(a as *const u16)
}
#[inline]
unsafe fn mr32(a: u64) -> u32 {
    core::ptr::read_volatile(a as *const u32)
}
#[inline]
unsafe fn mw8(a: u64, v: u8) {
    core::ptr::write_volatile(a as *mut u8, v)
}
#[inline]
unsafe fn mw16(a: u64, v: u16) {
    core::ptr::write_volatile(a as *mut u16, v)
}
#[inline]
unsafe fn mw32(a: u64, v: u32) {
    core::ptr::write_volatile(a as *mut u32, v)
}
#[inline]
unsafe fn mw64(a: u64, v: u64) {
    core::ptr::write_volatile(a as *mut u64, v)
}

/// One parsed virtio-pci capability: which BAR, byte offset, length, and (for
/// the notify cap) the notify-offset multiplier.
#[derive(Clone, Copy, Default)]
struct VirtioCap {
    bar: u8,
    offset: u32,
    length: u32,
    notify_mul: u32,
}

/// Walk the PCI capability list for the modern virtio structures.
fn parse_virtio_caps(dev: &crate::pci::PciDevice) -> (Option<VirtioCap>, Option<VirtioCap>) {
    let (b, d, f) = (dev.bus, dev.device, dev.function);
    // Status register bit 4 (0x10) => capability list present.
    let status = crate::pci::read_config_16(b, d, f, 0x06);
    if status & 0x10 == 0 {
        return (None, None);
    }
    let mut common = None;
    let mut notify = None;
    let mut cap_off = (crate::pci::read_config_8(b, d, f, 0x34) & 0xFC) as u8;
    let mut guard = 0;
    while cap_off != 0 && guard < 48 {
        guard += 1;
        let cap_id = crate::pci::read_config_8(b, d, f, cap_off);
        let next = crate::pci::read_config_8(b, d, f, cap_off + 1) & 0xFC;
        if cap_id == 0x09 {
            // vendor-specific (virtio)
            let cfg_type = crate::pci::read_config_8(b, d, f, cap_off + 3);
            let bar = crate::pci::read_config_8(b, d, f, cap_off + 4);
            let offset = crate::pci::read_config_32(b, d, f, cap_off + 8);
            let length = crate::pci::read_config_32(b, d, f, cap_off + 12);
            let mut cap = VirtioCap {
                bar,
                offset,
                length,
                notify_mul: 0,
            };
            match cfg_type {
                VIRTIO_PCI_CAP_COMMON_CFG => common = Some(cap),
                VIRTIO_PCI_CAP_NOTIFY_CFG => {
                    cap.notify_mul = crate::pci::read_config_32(b, d, f, cap_off + 16);
                    notify = Some(cap);
                }
                _ => {}
            }
        }
        cap_off = next;
    }
    (common, notify)
}

/// Map the BAR a virtio cap lives in and return the virt address of the cap's
/// structure (BAR base + cap.offset).
fn map_cap(dev: &crate::pci::PciDevice, cap: &VirtioCap) -> Option<u64> {
    let bar_phys = crate::pci::bar_address(dev, cap.bar)?;
    let bar_size = crate::pci::probe_bar_size(dev, cap.bar);
    let need = (cap.offset as u64).saturating_add(cap.length as u64);
    let size = core::cmp::max(bar_size, need);
    let base = crate::arch::mmu::kernel()
        .map_mmio_range(
            x86_64::PhysAddr::new(bar_phys),
            size as usize,
            crate::arch::mmu::PageFlags::DEVICE,
        )
        .as_u64();
    Some(base + cap.offset as u64)
}

// ── virtio-gpu command structs (all little-endian, repr C) ──────────────────
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CtrlHdr {
    type_: u32,
    flags: u32,
    fence_id: u64,
    ctx_id: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct DisplayOne {
    r: Rect,
    enabled: u32,
    flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RespDisplayInfo {
    hdr: CtrlHdr,
    pmodes: [DisplayOne; MAX_SCANOUTS],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ResourceCreate2d {
    hdr: CtrlHdr,
    resource_id: u32,
    format: u32,
    width: u32,
    height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct MemEntry {
    addr: u64,
    length: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ResourceAttachBacking {
    hdr: CtrlHdr,
    resource_id: u32,
    nr_entries: u32,
    // one inline MemEntry follows (we only ever attach a single contiguous page)
    entry: MemEntry,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SetScanout {
    hdr: CtrlHdr,
    r: Rect,
    scanout_id: u32,
    resource_id: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TransferToHost2d {
    hdr: CtrlHdr,
    r: Rect,
    offset: u64,
    resource_id: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ResourceFlush {
    hdr: CtrlHdr,
    r: Rect,
    resource_id: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CursorPos {
    scanout_id: u32,
    x: u32,
    y: u32,
    padding: u32,
}

/// virtio_gpu_update_cursor — carried on the cursor queue (queue 1) for both
/// UPDATE_CURSOR (set the cursor image from `resource_id` at `hot_x/hot_y`)
/// and MOVE_CURSOR (reposition; `resource_id` ignored). 56 bytes.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct UpdateCursor {
    hdr: CtrlHdr,
    pos: CursorPos,
    resource_id: u32,
    hot_x: u32,
    hot_y: u32,
    padding: u32,
}

// ── driver state for /proc + smoketest ──────────────────────────────────────
static GPU_PRESENT: AtomicBool = AtomicBool::new(false);
static CURSOR_RESULT: AtomicU32 = AtomicU32::new(0); // 0=unrun 1=pass 2=fail 3=skip
static GPU_QSIZE: AtomicU32 = AtomicU32::new(0);
static GPU_NUM_QUEUES: AtomicU32 = AtomicU32::new(0);
static DISP_WIDTH: AtomicU32 = AtomicU32::new(0);
static DISP_HEIGHT: AtomicU32 = AtomicU32::new(0);
static SMOKE_RESULT: AtomicU32 = AtomicU32::new(0); // 0=unrun 1=pass 2=fail
static CMDS_OK: AtomicU32 = AtomicU32::new(0);

/// A modern virtio-pci control queue, driven synchronously (one command in
/// flight — boot-time init, no IRQ). All device config access is MMIO via the
/// common-configuration + notification capability structures.
struct GpuQueue {
    notify_virt: u64,      // mapped notify cfg base + cap offset
    notify_mul: u32,       // notify_off_multiplier
    queue_notify_off: u16, // queue 0's notify offset
    queue_size: u16,
    desc: u64,  // virt addr of descriptor table
    avail: u64, // virt addr of avail ring
    used: u64,  // virt addr of used ring
    scratch_virt: u64,
    scratch_phys: u64,
    avail_idx: u16,
    last_used: u16,
}

const SCRATCH_CMD_OFF: u64 = 0;
const SCRATCH_RESP_OFF: u64 = 2048;

impl GpuQueue {
    /// Submit a command (device-readable) + response buffer (device-writable)
    /// as a 2-descriptor chain, notify, and poll the used ring for completion.
    /// Returns the response `CtrlHdr.type_`, or 0 on timeout.
    unsafe fn submit(&mut self, cmd: &[u8], cmd_len: usize, resp_len: usize) -> u32 {
        // Copy the command into scratch, clear the response area.
        core::ptr::copy_nonoverlapping(
            cmd.as_ptr(),
            (self.scratch_virt + SCRATCH_CMD_OFF) as *mut u8,
            cmd_len,
        );
        core::ptr::write_bytes(
            (self.scratch_virt + SCRATCH_RESP_OFF) as *mut u8,
            0,
            resp_len,
        );

        // desc[0]: device-readable command.
        let d0 = self.desc as *mut u8;
        write_desc(
            d0,
            self.scratch_phys + SCRATCH_CMD_OFF,
            cmd_len as u32,
            VRING_DESC_F_NEXT,
            1,
        );
        // desc[1]: device-writable response.
        let d1 = (self.desc + 16) as *mut u8;
        write_desc(
            d1,
            self.scratch_phys + SCRATCH_RESP_OFF,
            resp_len as u32,
            VRING_DESC_F_WRITE,
            0,
        );

        // avail ring: flags(2) idx(2) ring[].
        let avail_ring = (self.avail + 4) as *mut u16;
        core::ptr::write_volatile(
            avail_ring.add((self.avail_idx % self.queue_size) as usize),
            0,
        );
        core::sync::atomic::fence(Ordering::SeqCst);
        self.avail_idx = self.avail_idx.wrapping_add(1);
        let avail_idx_ptr = (self.avail + 2) as *mut u16;
        core::ptr::write_volatile(avail_idx_ptr, self.avail_idx);
        core::sync::atomic::fence(Ordering::SeqCst);

        // Notify queue 0 (modern: MMIO at notify_base + notify_off * multiplier).
        let notify_addr =
            self.notify_virt + (self.queue_notify_off as u64) * (self.notify_mul as u64);
        core::ptr::write_volatile(notify_addr as *mut u16, 0);

        // Poll the used ring idx (used: flags(2) idx(2) ring[]).
        let used_idx_ptr = (self.used + 2) as *const u16;
        let mut spins: u64 = 0;
        loop {
            let cur = core::ptr::read_volatile(used_idx_ptr);
            if cur != self.last_used {
                self.last_used = cur;
                break;
            }
            spins += 1;
            if spins > 50_000_000 {
                crate::serial_println!("[vgpu] submit timeout (no used-ring completion)");
                // u32::MAX is not a valid RESP_* code (those are 0x1100+), so
                // callers checking a specific RESP value still fail correctly,
                // and the cursor path can tell timeout from a len-0 completion.
                return u32::MAX;
            }
            core::hint::spin_loop();
        }

        // Read the response header type.
        let resp_hdr = (self.scratch_virt + SCRATCH_RESP_OFF) as *const CtrlHdr;
        (*resp_hdr).type_
    }

    /// Submit a command whose device response is len-0 (cursor queue): success
    /// is the used ring advancing, not a RESP_* type. Returns false only on
    /// timeout.
    unsafe fn submit_consumed(&mut self, cmd: &[u8], cmd_len: usize) -> bool {
        self.submit(cmd, cmd_len, core::mem::size_of::<CtrlHdr>()) != u32::MAX
    }

    /// Copy the full response (after a successful submit) into `out`.
    unsafe fn read_response(&self, out: *mut u8, len: usize) {
        core::ptr::copy_nonoverlapping(
            (self.scratch_virt + SCRATCH_RESP_OFF) as *const u8,
            out,
            len,
        );
    }
}

#[inline]
unsafe fn write_desc(d: *mut u8, addr: u64, len: u32, flags: u16, next: u16) {
    core::ptr::write_volatile(d as *mut u64, addr);
    core::ptr::write_volatile((d.add(8)) as *mut u32, len);
    core::ptr::write_volatile((d.add(12)) as *mut u16, flags);
    core::ptr::write_volatile((d.add(14)) as *mut u16, next);
}

#[inline]
unsafe fn as_bytes<T>(v: &T) -> &[u8] {
    core::slice::from_raw_parts(v as *const T as *const u8, core::mem::size_of::<T>())
}

static GPU_QUEUE: Mutex<Option<GpuQueue>> = Mutex::new(None);
/// virtio-gpu cursor queue (queue index 1). Same split-virtqueue mechanism as
/// the controlq, separate rings — carries UPDATE_CURSOR / MOVE_CURSOR for the
/// hardware cursor plane (MasterChecklist Phase 2.5).
static CURSOR_QUEUE: Mutex<Option<GpuQueue>> = Mutex::new(None);

/// Allocate + program one split virtqueue `index` over the modern common-cfg.
/// Mirrors the inline controlq setup; returns a ready `GpuQueue` or None.
unsafe fn setup_queue(
    common: u64,
    notify_mul: u32,
    notify_virt: u64,
    index: u16,
) -> Option<GpuQueue> {
    mw16(common + CC_QUEUE_SELECT, index);
    let qsz = mr16(common + CC_QUEUE_SIZE);
    let queue_size = if qsz == 0 || qsz > 256 { 0 } else { qsz };
    if queue_size == 0 {
        return None;
    }
    let offset = crate::memory::PHYS_MEM_OFFSET.get()?.as_u64();
    let ring_phys = crate::memory::allocate_contiguous_frames(2)?.as_u64();
    let ring_virt = ring_phys + offset;
    core::ptr::write_bytes(ring_virt as *mut u8, 0, 3 * 4096);
    // VIRTQ_AVAIL_F_NO_INTERRUPT — we poll the used ring.
    core::ptr::write_volatile((ring_virt + 4096) as *mut u16, 1u16);
    let scratch_phys = crate::memory::allocate_contiguous_frames(0)?.as_u64();
    let scratch_virt = scratch_phys + offset;
    core::ptr::write_bytes(scratch_virt as *mut u8, 0, 4096);
    mw64(common + CC_QUEUE_DESC, ring_phys);
    mw64(common + CC_QUEUE_DRIVER, ring_phys + 4096);
    mw64(common + CC_QUEUE_DEVICE, ring_phys + 8192);
    let queue_notify_off = mr16(common + CC_QUEUE_NOTIFY_OFF);
    mw16(common + CC_QUEUE_ENABLE, 1);
    Some(GpuQueue {
        notify_virt,
        notify_mul,
        queue_notify_off,
        queue_size,
        desc: ring_virt,
        avail: ring_virt + 4096,
        used: ring_virt + 8192,
        scratch_virt,
        scratch_phys,
        avail_idx: 0,
        last_used: 0,
    })
}

/// Locate the virtio-gpu PCI function (vendor 0x1AF4, Display class).
fn find_virtio_gpu() -> Option<crate::pci::PciDevice> {
    crate::pci::enumerate()
        .into_iter()
        .find(|d| d.vendor_id == 0x1AF4 && d.class == 0x03)
}

/// Bring the device up over the modern virtio-pci transport (PCI-capability
/// common-config + notify MMIO) and set up control queue 0.
pub fn init() {
    let dev = match find_virtio_gpu() {
        Some(d) => d,
        None => {
            crate::serial_println!("[vgpu] no virtio-gpu device found (skipping)");
            return;
        }
    };

    let (common_cap, notify_cap) = parse_virtio_caps(&dev);
    let (common_cap, notify_cap) = match (common_cap, notify_cap) {
        (Some(c), Some(n)) => (c, n),
        _ => {
            crate::serial_println!("[vgpu] missing virtio common/notify capabilities");
            return;
        }
    };

    let common = match map_cap(&dev, &common_cap) {
        Some(v) => v,
        None => {
            crate::serial_println!("[vgpu] failed to map common_cfg BAR");
            return;
        }
    };
    let notify_virt = match map_cap(&dev, &notify_cap) {
        Some(v) => v,
        None => {
            crate::serial_println!("[vgpu] failed to map notify BAR");
            return;
        }
    };

    // Disable legacy INTx (we drive the queue by polling — there is no handler).
    // Leaving INTx enabled with no ISR causes an interrupt storm once interrupts
    // flow after DRIVER_OK, which livelocks the kernel. Also ensure memory-space
    // (bit 1) + bus-master (bit 2) are on so MMIO config + ring DMA work.
    {
        let cmd = crate::pci::read_config_16(dev.bus, dev.device, dev.function, 0x04);
        let newcmd = cmd | (1 << 1) | (1 << 2) | (1 << 10); // mem | busmaster | INTx-disable
        crate::pci::write_config_16(dev.bus, dev.device, dev.function, 0x04, newcmd);
    }

    unsafe {
        // Reset, then ACKNOWLEDGE + DRIVER.
        mw8(common + CC_DEVICE_STATUS, 0);
        // Wait for the device to acknowledge reset (status reads back 0).
        let mut spin = 0;
        while mr8(common + CC_DEVICE_STATUS) != 0 && spin < 1_000_000 {
            spin += 1;
            core::hint::spin_loop();
        }
        mw8(common + CC_DEVICE_STATUS, S_ACK);
        mw8(common + CC_DEVICE_STATUS, S_ACK | S_DRIVER);

        // Feature negotiation: we MUST accept VIRTIO_F_VERSION_1 (bit 32).
        mw32(common + CC_DEVICE_FEATURE_SELECT, 1);
        let dev_feat_hi = mr32(common + CC_DEVICE_FEATURE);
        let has_v1 = dev_feat_hi & VIRTIO_F_VERSION_1_HI != 0;
        // Accept no low (device-type) features; accept only VERSION_1 high.
        mw32(common + CC_DRIVER_FEATURE_SELECT, 0);
        mw32(common + CC_DRIVER_FEATURE, 0);
        mw32(common + CC_DRIVER_FEATURE_SELECT, 1);
        mw32(common + CC_DRIVER_FEATURE, VIRTIO_F_VERSION_1_HI);

        // FEATURES_OK and confirm the device still accepts our set.
        mw8(common + CC_DEVICE_STATUS, S_ACK | S_DRIVER | S_FEATURES_OK);
        let features_ok = mr8(common + CC_DEVICE_STATUS) & S_FEATURES_OK != 0;
        if !has_v1 || !features_ok {
            mw8(common + CC_DEVICE_STATUS, S_FAILED);
            crate::serial_println!(
                "[vgpu] feature negotiation failed (v1={} features_ok={})",
                has_v1,
                features_ok
            );
            return;
        }

        // Set up control queue 0.
        mw16(common + CC_QUEUE_SELECT, 0);
        let qsz = mr16(common + CC_QUEUE_SIZE);
        let queue_size = if qsz == 0 || qsz > 256 { 0 } else { qsz };
        if queue_size == 0 {
            mw8(common + CC_DEVICE_STATUS, S_FAILED);
            crate::serial_println!("[vgpu] controlq size {} unusable", qsz);
            return;
        }

        // 3 contiguous pages (desc | avail | used) → buddy order 2 (4 frames).
        let offset = match crate::memory::PHYS_MEM_OFFSET.get() {
            Some(o) => o.as_u64(),
            None => return,
        };
        let ring_phys = match crate::memory::allocate_contiguous_frames(2) {
            Some(p) => p.as_u64(),
            None => return,
        };
        let ring_virt = ring_phys + offset;
        core::ptr::write_bytes(ring_virt as *mut u8, 0, 3 * 4096);
        // VIRTQ_AVAIL_F_NO_INTERRUPT: we poll the used ring, so tell the device
        // never to raise a used-ring interrupt (belt-and-suspenders with the
        // PCI INTx-disable above).
        core::ptr::write_volatile((ring_virt + 4096) as *mut u16, 1u16);

        let scratch_phys = match crate::memory::allocate_contiguous_frames(0) {
            Some(p) => p.as_u64(),
            None => return,
        };
        let scratch_virt = scratch_phys + offset;
        core::ptr::write_bytes(scratch_virt as *mut u8, 0, 4096);

        // Point the device at our split-virtqueue rings (separate phys addrs).
        mw64(common + CC_QUEUE_DESC, ring_phys);
        mw64(common + CC_QUEUE_DRIVER, ring_phys + 4096);
        mw64(common + CC_QUEUE_DEVICE, ring_phys + 8192);
        let queue_notify_off = mr16(common + CC_QUEUE_NOTIFY_OFF);
        mw16(common + CC_QUEUE_ENABLE, 1);

        *GPU_QUEUE.lock() = Some(GpuQueue {
            notify_virt,
            notify_mul: notify_cap.notify_mul,
            queue_notify_off,
            queue_size,
            desc: ring_virt,
            avail: ring_virt + 4096,
            used: ring_virt + 8192,
            scratch_virt,
            scratch_phys,
            avail_idx: 0,
            last_used: 0,
        });

        // Cursor queue (index 1) — virtio-gpu always exposes it. Set it up
        // before DRIVER_OK so the hardware-cursor plane is live immediately
        // (MasterChecklist Phase 2.5). A missing/unusable cursorq is
        // non-fatal: the controlq still presents.
        let num_queues = mr16(common + CC_NUM_QUEUES);
        if num_queues >= 2 {
            match setup_queue(common, notify_cap.notify_mul, notify_virt, 1) {
                Some(cq) => *CURSOR_QUEUE.lock() = Some(cq),
                None => {
                    crate::serial_println!("[vgpu] cursorq unusable (continuing without HW cursor)")
                }
            }
        }

        // DRIVER_OK — device is live.
        mw8(
            common + CC_DEVICE_STATUS,
            S_ACK | S_DRIVER | S_FEATURES_OK | S_DRIVER_OK,
        );

        GPU_QSIZE.store(queue_size as u32, Ordering::Relaxed);
        GPU_NUM_QUEUES.store(num_queues as u32, Ordering::Relaxed);
    }

    GPU_PRESENT.store(true, Ordering::Relaxed);
    crate::serial_println!(
        "[vgpu] virtio-gpu up (modern): {:04x}:{:04x} qsize={} notify_mul={}",
        dev.vendor_id,
        dev.device_id,
        GPU_QSIZE.load(Ordering::Relaxed),
        notify_cap.notify_mul,
    );
}

/// Query scanout 0's dimensions via GET_DISPLAY_INFO. Returns `(w, h)` or `None`.
fn get_display_info() -> Option<(u32, u32)> {
    let mut guard = GPU_QUEUE.lock();
    let q = guard.as_mut()?;
    let cmd = CtrlHdr {
        type_: CMD_GET_DISPLAY_INFO,
        ..Default::default()
    };
    let resp_len = core::mem::size_of::<RespDisplayInfo>();
    let rtype = unsafe { q.submit(as_bytes(&cmd), core::mem::size_of::<CtrlHdr>(), resp_len) };
    if rtype != RESP_OK_DISPLAY_INFO {
        crate::serial_println!("[vgpu] GET_DISPLAY_INFO bad resp {:#x}", rtype);
        return None;
    }
    let mut info = RespDisplayInfo {
        hdr: CtrlHdr::default(),
        pmodes: [DisplayOne::default(); MAX_SCANOUTS],
    };
    unsafe { q.read_response(&mut info as *mut _ as *mut u8, resp_len) };
    let m = info.pmodes[0];
    if m.enabled == 0 || m.r.width == 0 {
        crate::serial_println!("[vgpu] scanout 0 not enabled");
        return None;
    }
    Some((m.r.width, m.r.height))
}

/// Run a command and require an OK_NODATA response. Returns true on success.
fn submit_ok(bytes: &[u8], len: usize) -> bool {
    let mut guard = GPU_QUEUE.lock();
    let q = match guard.as_mut() {
        Some(q) => q,
        None => return false,
    };
    let resp_len = core::mem::size_of::<CtrlHdr>();
    let rtype = unsafe { q.submit(bytes, len, resp_len) };
    let ok = rtype == RESP_OK_NODATA;
    if ok {
        CMDS_OK.fetch_add(1, Ordering::Relaxed);
    } else {
        crate::serial_println!("[vgpu] command resp {:#x} (expected OK_NODATA)", rtype);
    }
    ok
}

/// Full present path against a small backed resource: create → attach backing →
/// set scanout → transfer → flush. Proves the GPU presents from a resource (the
/// device-side equivalent of a hardware page-flip). Returns true if every step
/// returned OK. Uses a modest 256×64 resource so the backing fits one frame.
fn present_smoketest(disp_w: u32, disp_h: u32) -> bool {
    const W: u32 = 256;
    const H: u32 = 64;
    const RID: u32 = 1;
    let bytes = (W * H * 4) as usize;

    // Backing store: one contiguous region, filled with an opaque blue.
    // `allocate_contiguous_frames` takes a buddy ORDER (2^order frames), so
    // pick the smallest order that covers the backing's page count.
    let pages = (bytes + 4095) / 4096;
    let mut order: u8 = 0;
    while (1usize << order) < pages {
        order += 1;
    }
    let (fb_phys, fb_virt) = unsafe {
        let phys = match crate::memory::allocate_contiguous_frames(order) {
            Some(p) => p.as_u64(),
            None => return false,
        };
        let off = match crate::memory::PHYS_MEM_OFFSET.get() {
            Some(o) => o.as_u64(),
            None => return false,
        };
        let virt = phys + off;
        // BGRA: opaque AthenaOS blue.
        let px: u32 = 0xFF_1E_3A_5F;
        let p = virt as *mut u32;
        for i in 0..(W * H) as usize {
            core::ptr::write_volatile(p.add(i), px);
        }
        (phys, virt)
    };
    let _ = fb_virt;

    // 1. RESOURCE_CREATE_2D
    let create = ResourceCreate2d {
        hdr: CtrlHdr {
            type_: CMD_RESOURCE_CREATE_2D,
            ..Default::default()
        },
        resource_id: RID,
        format: FORMAT_B8G8R8A8_UNORM,
        width: W,
        height: H,
    };
    if !submit_ok(
        unsafe { as_bytes(&create) },
        core::mem::size_of::<ResourceCreate2d>(),
    ) {
        return false;
    }

    // 2. RESOURCE_ATTACH_BACKING (single contiguous entry).
    let attach = ResourceAttachBacking {
        hdr: CtrlHdr {
            type_: CMD_RESOURCE_ATTACH_BACKING,
            ..Default::default()
        },
        resource_id: RID,
        nr_entries: 1,
        entry: MemEntry {
            addr: fb_phys,
            length: bytes as u32,
            padding: 0,
        },
    };
    if !submit_ok(
        unsafe { as_bytes(&attach) },
        core::mem::size_of::<ResourceAttachBacking>(),
    ) {
        return false;
    }

    // 3. SET_SCANOUT — bind resource to scanout 0 (use full resource rect).
    let scanout = SetScanout {
        hdr: CtrlHdr {
            type_: CMD_SET_SCANOUT,
            ..Default::default()
        },
        r: Rect {
            x: 0,
            y: 0,
            width: W.min(disp_w.max(1)),
            height: H.min(disp_h.max(1)),
        },
        scanout_id: 0,
        resource_id: RID,
    };
    if !submit_ok(
        unsafe { as_bytes(&scanout) },
        core::mem::size_of::<SetScanout>(),
    ) {
        return false;
    }

    // 4. TRANSFER_TO_HOST_2D — upload the backing into the host resource.
    let xfer = TransferToHost2d {
        hdr: CtrlHdr {
            type_: CMD_TRANSFER_TO_HOST_2D,
            ..Default::default()
        },
        r: Rect {
            x: 0,
            y: 0,
            width: W,
            height: H,
        },
        offset: 0,
        resource_id: RID,
        padding: 0,
    };
    if !submit_ok(
        unsafe { as_bytes(&xfer) },
        core::mem::size_of::<TransferToHost2d>(),
    ) {
        return false;
    }

    // 5. RESOURCE_FLUSH — present (the "page flip").
    let flush = ResourceFlush {
        hdr: CtrlHdr {
            type_: CMD_RESOURCE_FLUSH,
            ..Default::default()
        },
        r: Rect {
            x: 0,
            y: 0,
            width: W,
            height: H,
        },
        resource_id: RID,
        padding: 0,
    };
    submit_ok(
        unsafe { as_bytes(&flush) },
        core::mem::size_of::<ResourceFlush>(),
    )
}

// ─── Live full-screen scanout (the compositor's ScanoutBackend::VirtioGpu) ──
// Distinct from `present_smoketest` (a 256x64 proof): a persistent display-sized
// resource the compositor transfers each composited frame into, then flushes —
// the device-side page-flip. Backing virt addr 0 = "not initialised".
static LIVE_FB_VIRT: AtomicU64 = AtomicU64::new(0);
static LIVE_W: AtomicU32 = AtomicU32::new(0);
static LIVE_H: AtomicU32 = AtomicU32::new(0);
const LIVE_RID: u32 = 0x10; // distinct from the smoketest (1) + cursor resources

/// True once a virtio-gpu device has been brought up (controlq is live).
pub fn is_available() -> bool {
    GPU_QUEUE.lock().is_some()
}

/// Create the persistent `w`x`h` scanout resource, allocate + attach its
/// backing, and bind it to scanout 0. Idempotent. Returns true only if the
/// device is present and every controlq step succeeded.
pub fn present_init(w: u32, h: u32) -> bool {
    if w == 0 || h == 0 || GPU_QUEUE.lock().is_none() {
        return false;
    }
    if LIVE_FB_VIRT.load(Ordering::Relaxed) != 0 {
        return true; // already initialised
    }
    let bytes = (w as usize) * (h as usize) * 4;
    let pages = (bytes + 4095) / 4096;
    let mut order: u8 = 0;
    while (1usize << order) < pages {
        order += 1;
    }
    let (fb_phys, fb_virt) = {
        let phys = match crate::memory::allocate_contiguous_frames(order) {
            Some(p) => p.as_u64(),
            None => return false,
        };
        let off = match crate::memory::PHYS_MEM_OFFSET.get() {
            Some(o) => o.as_u64(),
            None => return false,
        };
        (phys, phys + off)
    };
    unsafe {
        core::ptr::write_bytes(fb_virt as *mut u8, 0, bytes);
    }
    let create = ResourceCreate2d {
        hdr: CtrlHdr {
            type_: CMD_RESOURCE_CREATE_2D,
            ..Default::default()
        },
        resource_id: LIVE_RID,
        format: FORMAT_B8G8R8A8_UNORM,
        width: w,
        height: h,
    };
    if !submit_ok(
        unsafe { as_bytes(&create) },
        core::mem::size_of::<ResourceCreate2d>(),
    ) {
        return false;
    }
    let attach = ResourceAttachBacking {
        hdr: CtrlHdr {
            type_: CMD_RESOURCE_ATTACH_BACKING,
            ..Default::default()
        },
        resource_id: LIVE_RID,
        nr_entries: 1,
        entry: MemEntry {
            addr: fb_phys,
            length: bytes as u32,
            padding: 0,
        },
    };
    if !submit_ok(
        unsafe { as_bytes(&attach) },
        core::mem::size_of::<ResourceAttachBacking>(),
    ) {
        return false;
    }
    let scanout = SetScanout {
        hdr: CtrlHdr {
            type_: CMD_SET_SCANOUT,
            ..Default::default()
        },
        r: Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        },
        scanout_id: 0,
        resource_id: LIVE_RID,
    };
    if !submit_ok(
        unsafe { as_bytes(&scanout) },
        core::mem::size_of::<SetScanout>(),
    ) {
        return false;
    }
    LIVE_W.store(w, Ordering::Relaxed);
    LIVE_H.store(h, Ordering::Relaxed);
    LIVE_FB_VIRT.store(fb_virt, Ordering::Relaxed);
    crate::serial_println!("[vgpu] live scanout ready: {}x{} (rid {})", w, h, LIVE_RID);
    true
}

/// Present one composited frame: copy `comp` (ARGB u32 == B8G8R8A8 little-endian,
/// so a direct copy) into the backing, then TRANSFER_TO_HOST_2D + RESOURCE_FLUSH
/// (the page-flip). No-op if not initialised or the dimensions don't match.
pub fn present_frame(comp: &[u32], w: u32, h: u32) {
    let virt = LIVE_FB_VIRT.load(Ordering::Relaxed);
    if virt == 0 || w != LIVE_W.load(Ordering::Relaxed) || h != LIVE_H.load(Ordering::Relaxed) {
        return;
    }
    let n = (w as usize) * (h as usize);
    if comp.len() < n {
        return;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(comp.as_ptr(), virt as *mut u32, n);
    }
    let xfer = TransferToHost2d {
        hdr: CtrlHdr {
            type_: CMD_TRANSFER_TO_HOST_2D,
            ..Default::default()
        },
        r: Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        },
        offset: 0,
        resource_id: LIVE_RID,
        padding: 0,
    };
    let _ = submit_ok(
        unsafe { as_bytes(&xfer) },
        core::mem::size_of::<TransferToHost2d>(),
    );
    let flush = ResourceFlush {
        hdr: CtrlHdr {
            type_: CMD_RESOURCE_FLUSH,
            ..Default::default()
        },
        r: Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        },
        resource_id: LIVE_RID,
        padding: 0,
    };
    let _ = submit_ok(
        unsafe { as_bytes(&flush) },
        core::mem::size_of::<ResourceFlush>(),
    );
}

/// Phase 2.5 hardware-cursor proof: create a 64×64 cursor resource on the
/// controlq, then drive UPDATE_CURSOR + MOVE_CURSOR on the CURSOR queue
/// (queue 1) and confirm the device consumes each off the used ring. This
/// exercises the dedicated cursor plane path — distinct from the controlq
/// scanout path — so cursor motion never waits behind frame submission.
fn cursor_smoketest() -> bool {
    const CRID: u32 = 64; // cursor resource id (distinct from the present RID)
    const CW: u32 = 64;
    const CH: u32 = 64;
    let bytes = (CW * CH * 4) as usize;

    // Backing for the cursor image (one page covers 64×64×4 = 16 KiB → order 2).
    let pages = (bytes + 4095) / 4096;
    let mut order: u8 = 0;
    while (1usize << order) < pages {
        order += 1;
    }
    let (cur_phys, _cur_virt) = unsafe {
        let phys = match crate::memory::allocate_contiguous_frames(order) {
            Some(p) => p.as_u64(),
            None => return false,
        };
        let off = match crate::memory::PHYS_MEM_OFFSET.get() {
            Some(o) => o.as_u64(),
            None => return false,
        };
        let virt = phys + off;
        // Opaque white arrow pixel fill (content irrelevant to the path proof).
        let p = virt as *mut u32;
        for i in 0..(CW * CH) as usize {
            core::ptr::write_volatile(p.add(i), 0xFF_FF_FF_FF);
        }
        (phys, virt)
    };

    // Cursor resource lives on the controlq (create + attach + upload).
    let create = ResourceCreate2d {
        hdr: CtrlHdr {
            type_: CMD_RESOURCE_CREATE_2D,
            ..Default::default()
        },
        resource_id: CRID,
        format: FORMAT_B8G8R8A8_UNORM,
        width: CW,
        height: CH,
    };
    if !submit_ok(
        unsafe { as_bytes(&create) },
        core::mem::size_of::<ResourceCreate2d>(),
    ) {
        return false;
    }
    let attach = ResourceAttachBacking {
        hdr: CtrlHdr {
            type_: CMD_RESOURCE_ATTACH_BACKING,
            ..Default::default()
        },
        resource_id: CRID,
        nr_entries: 1,
        entry: MemEntry {
            addr: cur_phys,
            length: bytes as u32,
            padding: 0,
        },
    };
    if !submit_ok(
        unsafe { as_bytes(&attach) },
        core::mem::size_of::<ResourceAttachBacking>(),
    ) {
        return false;
    }
    let xfer = TransferToHost2d {
        hdr: CtrlHdr {
            type_: CMD_TRANSFER_TO_HOST_2D,
            ..Default::default()
        },
        r: Rect {
            x: 0,
            y: 0,
            width: CW,
            height: CH,
        },
        offset: 0,
        resource_id: CRID,
        padding: 0,
    };
    if !submit_ok(
        unsafe { as_bytes(&xfer) },
        core::mem::size_of::<TransferToHost2d>(),
    ) {
        return false;
    }

    // Now the cursor-queue commands.
    let mut cq = CURSOR_QUEUE.lock();
    let cq = match cq.as_mut() {
        Some(q) => q,
        None => return false,
    };

    // UPDATE_CURSOR: bind the resource at hotspot (0,0), placed at (16,16).
    let update = UpdateCursor {
        hdr: CtrlHdr {
            type_: CMD_UPDATE_CURSOR,
            ..Default::default()
        },
        pos: CursorPos {
            scanout_id: 0,
            x: 16,
            y: 16,
            padding: 0,
        },
        resource_id: CRID,
        hot_x: 0,
        hot_y: 0,
        padding: 0,
    };
    let update_ok =
        unsafe { cq.submit_consumed(as_bytes(&update), core::mem::size_of::<UpdateCursor>()) };

    // MOVE_CURSOR: reposition only (resource_id ignored by the device).
    let mv = UpdateCursor {
        hdr: CtrlHdr {
            type_: CMD_MOVE_CURSOR,
            ..Default::default()
        },
        pos: CursorPos {
            scanout_id: 0,
            x: 200,
            y: 120,
            padding: 0,
        },
        resource_id: 0,
        hot_x: 0,
        hot_y: 0,
        padding: 0,
    };
    let move_ok =
        unsafe { cq.submit_consumed(as_bytes(&mv), core::mem::size_of::<UpdateCursor>()) };

    update_ok && move_ok
}

/// R10 boot smoketest: prove the GPU command path end-to-end.
pub fn run_boot_smoketest() {
    if !GPU_PRESENT.load(Ordering::Relaxed) {
        crate::serial_println!("[vgpu] smoketest: skipped (no virtio-gpu device)");
        return;
    }

    let disp = get_display_info();
    let (w, h) = match disp {
        Some((w, h)) => {
            DISP_WIDTH.store(w, Ordering::Relaxed);
            DISP_HEIGHT.store(h, Ordering::Relaxed);
            (w, h)
        }
        None => (0, 0),
    };

    let present_ok = if disp.is_some() {
        present_smoketest(w, h)
    } else {
        false
    };

    let pass = disp.is_some() && present_ok;
    SMOKE_RESULT.store(if pass { 1 } else { 2 }, Ordering::Relaxed);
    crate::serial_println!(
        "[vgpu] smoketest: display_info={} ({}x{}) present_path_ok={} cmds_ok={} -> {}",
        disp.is_some(),
        w,
        h,
        present_ok,
        CMDS_OK.load(Ordering::Relaxed),
        if pass { "PASS" } else { "FAIL" }
    );

    // Phase 2.5: hardware cursor on the dedicated cursor queue.
    let has_cursorq = CURSOR_QUEUE.lock().is_some();
    if has_cursorq {
        let cursor_ok = cursor_smoketest();
        CURSOR_RESULT.store(if cursor_ok { 1 } else { 2 }, Ordering::Relaxed);
        crate::serial_println!(
            "[vgpu] cursor smoketest: cursorq=present update_cursor={} move_cursor={} -> {}",
            cursor_ok,
            cursor_ok,
            if cursor_ok { "PASS" } else { "FAIL" }
        );
    } else {
        CURSOR_RESULT.store(3, Ordering::Relaxed);
        crate::serial_println!("[vgpu] cursor smoketest: no cursor queue -> SKIP");
    }
}

/// `/proc/raeen/virtio_gpu` body.
pub fn dump_text() -> String {
    format!(
        "# virtio-gpu (Phase 6 CPU→GPU on-ramp, modern virtio-pci)\n\
         present:      {}\n\
         controlq_size: {}\n\
         num_queues:   {}\n\
         display:      {}x{}\n\
         present_cmds_ok: {}\n\
         smoketest:    {}\n\
         hw_cursor:    {}\n",
        GPU_PRESENT.load(Ordering::Relaxed),
        GPU_QSIZE.load(Ordering::Relaxed),
        GPU_NUM_QUEUES.load(Ordering::Relaxed),
        DISP_WIDTH.load(Ordering::Relaxed),
        DISP_HEIGHT.load(Ordering::Relaxed),
        CMDS_OK.load(Ordering::Relaxed),
        match SMOKE_RESULT.load(Ordering::Relaxed) {
            1 => "PASS",
            2 => "FAIL",
            _ => "not run",
        },
        match CURSOR_RESULT.load(Ordering::Relaxed) {
            1 => "PASS",
            2 => "FAIL",
            3 => "skip (no cursorq)",
            _ => "not run",
        }
    )
}
