//! USB Mass Storage Class (MSC) — Bulk-Only Transport (BOT) driver.
//!
//! Concept §Storage: "AthenaOS supports USB storage as a hot-pluggable block
//! device source"; this module is the in-kernel MSC layer that sits between
//! xHCI and the block_io BlockDevice trait.
//!
//! ## Protocol overview
//!
//! USB MSC BOT uses a three-phase exchange per command:
//!   1. CBW (31 B, host → device, bulk-OUT): SCSI command wrapped in a
//!      USB-MSC envelope (signature 0x43425355).
//!   2. Data phase (bulk-IN for reads, bulk-OUT for writes): the actual payload.
//!   3. CSW (13 B, device → host, bulk-IN): completion status (0=success).
//!
//! ## QEMU note
//!
//! The xtask QEMU command attaches a `usb-storage` device backed by
//! `target/usb-msc.img` (`-drive if=none,id=usbmsc,... -device usb-storage,
//! drive=usbmsc,bus=xhci.0,port=4`). The image carries a signature at sector 0
//! so the boot smoketest can verify a real READ(10) landed. On a stock QEMU
//! boot with no `usb-storage` device the smoketest finds zero MSC devices,
//! which is also a PASS.
//!
//! ## Bring-up flow
//!
//! 1. `xhci::bring_up_msc` (during enumeration) configures the bulk IN/OUT
//!    endpoints (Configure Endpoint) + SET_CONFIGURATION.
//! 2. `probe_all_msc` walks the config descriptor, finds the bulk endpoints,
//!    and runs READ_CAPACITY(10) to populate geometry.
//! 3. Each device is registered as a `BlockDevice` for the partition scanner.
//!
//! ## MasterChecklist reference
//!
//! MasterChecklist Phase 2.1 / 3.4 — USB MSC boot-device enumeration.

#![allow(dead_code)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;
use x86_64::structures::paging::{FrameAllocator, PhysFrame, Size4KiB};

use crate::block_io::BlockDevice;

// ─── BOT Protocol Constants ──────────────────────────────────────────────────

/// CBW signature: little-endian "USBC"
const CBW_SIGNATURE: u32 = 0x4342_5355;
/// CSW signature: little-endian "USBS"
const CSW_SIGNATURE: u32 = 0x5342_5355;

const CBW_FLAG_DATA_IN: u8 = 0x80; // device → host  (read)
const CBW_FLAG_DATA_OUT: u8 = 0x00; // host → device  (write)

const CSW_STATUS_GOOD: u8 = 0;
const CSW_STATUS_FAILED: u8 = 1;
const CSW_STATUS_PHASE_ERROR: u8 = 2;

const CBW_LEN: usize = 31;
const CSW_LEN: usize = 13;

// SCSI operation codes
const SCSI_INQUIRY: u8 = 0x12;
const SCSI_READ_CAPACITY: u8 = 0x25; // READ_CAPACITY(10)
const SCSI_READ_10: u8 = 0x28;
const SCSI_WRITE_10: u8 = 0x2A;
const SCSI_TEST_UNIT_READY: u8 = 0x00;

// USB interface class / subclass / protocol for MSC BOT
const USB_CLASS_MSC: u8 = 0x08;
const USB_SUBCLASS_SCSI: u8 = 0x06;
const USB_PROTO_BOT: u8 = 0x50;

// ─── CBW / CSW Structs ───────────────────────────────────────────────────────

/// Command Block Wrapper — 31 bytes, USB MSC BOT spec §5.1.
///
/// Packed representation exactly matches the on-wire layout.
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Cbw {
    /// Must be 0x43425355 ("USBC") little-endian.
    signature: u32,
    /// Host-assigned tag; CSW must echo it back.
    tag: u32,
    /// Bytes the host expects to transfer in the data phase.
    data_transfer_length: u32,
    /// 0x80 = data IN (device → host), 0x00 = data OUT (host → device).
    flags: u8,
    /// Logical Unit Number (0 for single-LUN devices).
    lun: u8,
    /// Meaningful bytes in `cb` (6 or 10 for standard SCSI-2 CDBs).
    cb_length: u8,
    /// SCSI Command Descriptor Block, zero-padded to 16 bytes.
    cb: [u8; 16],
}

impl Cbw {
    fn new(tag: u32, data_len: u32, flags: u8, lun: u8, cdb: &[u8]) -> Self {
        let mut cb = [0u8; 16];
        let len = cdb.len().min(16);
        cb[..len].copy_from_slice(&cdb[..len]);
        Cbw {
            signature: CBW_SIGNATURE,
            tag,
            data_transfer_length: data_len,
            flags,
            lun,
            cb_length: len as u8,
            cb,
        }
    }

    fn as_bytes(&self) -> [u8; CBW_LEN] {
        let mut out = [0u8; CBW_LEN];
        out[0..4].copy_from_slice(&self.signature.to_le_bytes());
        out[4..8].copy_from_slice(&self.tag.to_le_bytes());
        out[8..12].copy_from_slice(&self.data_transfer_length.to_le_bytes());
        out[12] = self.flags;
        out[13] = self.lun;
        out[14] = self.cb_length;
        out[15..31].copy_from_slice(&self.cb);
        out
    }
}

/// Command Status Wrapper — 13 bytes, USB MSC BOT spec §5.2.
#[derive(Clone, Copy, Default)]
struct Csw {
    /// Must be 0x53425355 ("USBS") little-endian.
    signature: u32,
    /// Must match the corresponding CBW tag.
    tag: u32,
    /// Bytes not transferred (0 means all bytes were moved as expected).
    data_residue: u32,
    /// 0 = success, 1 = command failed, 2 = phase error.
    status: u8,
}

impl Csw {
    fn from_bytes(b: &[u8; CSW_LEN]) -> Self {
        Csw {
            signature: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            tag: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            data_residue: u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
            status: b[12],
        }
    }

    fn is_valid(&self, expected_tag: u32) -> bool {
        self.signature == CSW_SIGNATURE && self.tag == expected_tag
    }
}

fn alloc_dma_page() -> Result<(u64, u64, PhysFrame<Size4KiB>), &'static str> {
    let mut alloc = crate::memory::GlobalFrameAllocator;
    let frame = alloc
        .allocate_frame()
        .ok_or("usb-msc: OOM allocating DMA page")?;
    let phys = frame.start_address().as_u64();
    let virt = crate::memory::phys_to_virt(phys).as_u64();
    unsafe {
        core::ptr::write_bytes(virt as *mut u8, 0, 4096);
    }
    Ok((phys, virt, frame))
}

fn free_dma_page(frame: PhysFrame<Size4KiB>) {
    crate::memory::deallocate_frame(frame);
}

/// Return a bulk-transfer DMA page to the allocator ONLY when the controller
/// can no longer write into it.
///
/// `submitted` = the transfer TRB was actually posted to the controller's ring;
/// `completed` = a completion event was observed within the timeout.
///
/// The dangerous case is `submitted && !completed` (a 5 ms timeout): the TRB is
/// still live on the transfer ring, so the device may DMA into this page LATER —
/// e.g. a late 13-byte CSW whose little-endian signature is "USBS"
/// (`0x53425355`). If the page has already been returned to the buddy allocator
/// by then, that write lands in freed memory and overwrites the allocator's
/// intrusive free-list `next` pointer, so the next `pop_from_list` dereferences a
/// garbage pointer (observed: a ring-0 #PF to `0x..5342535d`, the corrupted
/// pointer, killing user_init and silently hanging ~80% of boots). That is a DMA
/// use-after-free corrupting the heap. To avoid it we LEAK the page in exactly
/// that case — bounded, because timeouts only happen during device enumeration —
/// rather than corrupt the allocator. (A future cancel-and-drain of the endpoint
/// ring could reclaim it; until then, correctness over the leaked 4 KiB.)
fn release_dma_page(
    frame: PhysFrame<Size4KiB>,
    phys: u64,
    slot: u8,
    submitted: bool,
    completed: bool,
) {
    if !submitted || completed {
        free_dma_page(frame);
    } else {
        crate::serial_println!(
            "[usb-msc] WARN slot {}: bulk transfer posted but did not complete in 5ms; \
             leaking DMA page {:#x} to avoid a late-DMA use-after-free (was corrupting the \
             allocator free list)",
            slot,
            phys
        );
    }
}

/// Map a USB endpoint address to the xHCI `transfer_rings[]`/`endpoints[]`
/// array index (DCI − 1) — the same convention the controller's
/// `submit_bulk_transfer` / doorbell path uses. (Previously this returned the
/// raw DCI, an off-by-one that targeted the wrong endpoint ring.)
fn ep_index_for(ep_addr: u8) -> u8 {
    crate::xhci_desc::xhci_ep_index(ep_addr)
}

// ─── Device Struct ───────────────────────────────────────────────────────────

/// A probed USB Mass Storage device.
///
/// One `UsbMscDevice` corresponds to a single logical unit (LUN) on a single
/// xHCI device slot.  Multi-LUN devices will have one `UsbMscDevice` per LUN
/// in `MSC_DEVICES`.
pub struct UsbMscDevice {
    /// Which xHCI controller owns `slot` (0 = primary, 1.. = secondaries —
    /// see `xhci::with_controller`). Slots are per-controller, so a transfer
    /// sent to the wrong xHC targets an unrelated device or nothing.
    pub controller: usize,
    /// xHCI device slot (1-based).
    pub slot: u8,
    /// Number of LUNs reported by GET_MAX_LUN (usually 0, meaning 1 LUN).
    pub lun_count: u8,
    /// Bulk-IN endpoint address (bit 7 set, e.g. 0x81).
    pub bulk_in_ep: u8,
    /// Bulk-OUT endpoint address (bit 7 clear, e.g. 0x02).
    pub bulk_out_ep: u8,
    /// Total addressable 512-byte (or `block_size`-byte) blocks.
    pub block_count: u64,
    /// Bytes per logical block (almost always 512 or 4096).
    pub block_size: u32,
    /// Monotonically incrementing tag for CBW/CSW correlation.
    tag: u32,
}

impl UsbMscDevice {
    fn next_tag(&mut self) -> u32 {
        self.tag = self.tag.wrapping_add(1);
        self.tag
    }

    // ── Low-level BOT helpers ────────────────────────────────────────────────

    /// Send a CBW over the bulk-OUT endpoint.
    ///
    /// In a fully wired build this would call
    /// `xhci::XHCI_CONTROLLER.lock().submit_bulk_transfer(self.slot,
    ///  ep_index_for(self.bulk_out_ep), phys_ptr, CBW_LEN as u32)`.
    /// We compute the on-wire bytes correctly; the actual DMA submission
    /// is gated behind the xHCI lock and requires a live device, which
    /// QEMU does not provide during boot.
    fn send_cbw(&mut self, cbw: &Cbw) -> Result<(), &'static str> {
        let bytes = cbw.as_bytes();
        let (phys, virt, frame) = alloc_dma_page()?;
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), virt as *mut u8, CBW_LEN);
        }
        let ep_idx = ep_index_for(self.bulk_out_ep);
        let (submitted, res) = crate::xhci::with_controller(self.controller, |ctrl| {
            match ctrl.submit_bulk_transfer(self.slot, ep_idx, phys, CBW_LEN as u32) {
                Err(e) => (false, Err(e)),
                Ok(_) => (true, ctrl.wait_for_transfer(5_000_000)),
            }
        })
        .unwrap_or((false, Err(crate::xhci::XhciError::NotReady)));
        release_dma_page(frame, phys, self.slot, submitted, res.is_ok());
        res.map(|_| ()).map_err(|_| "usb-msc: send_cbw failed")
    }

    /// Receive `len` bytes from the bulk-IN endpoint into `buf`.
    fn recv_bulk_in(&mut self, buf: &mut [u8], len: usize) -> Result<(), &'static str> {
        if len > 4096 {
            return Err("usb-msc: transfer > 4096 not supported yet");
        }
        if len == 0 {
            return Ok(());
        }
        let (phys, virt, frame) = alloc_dma_page()?;
        let ep_idx = ep_index_for(self.bulk_in_ep);

        let (submitted, res) = crate::xhci::with_controller(self.controller, |ctrl| {
            match ctrl.submit_bulk_transfer(self.slot, ep_idx, phys, len as u32) {
                Err(e) => (false, Err(e)),
                Ok(_) => (true, ctrl.wait_for_transfer(5_000_000)),
            }
        })
        .unwrap_or((false, Err(crate::xhci::XhciError::NotReady)));

        if let Ok(event) = res {
            let residual = (event.status & 0x00FF_FFFF) as usize;
            let got = len.saturating_sub(residual);
            let copy_len = got.min(buf.len());
            unsafe {
                core::ptr::copy_nonoverlapping(virt as *const u8, buf.as_mut_ptr(), copy_len);
            }
        }

        release_dma_page(frame, phys, self.slot, submitted, res.is_ok());
        res.map(|_| ()).map_err(|_| "usb-msc: recv_bulk_in failed")
    }

    /// Receive the 13-byte CSW from the bulk-IN endpoint.
    fn recv_csw(&mut self, expected_tag: u32) -> Result<Csw, &'static str> {
        let (phys, virt, frame) = alloc_dma_page()?;
        let ep_idx = ep_index_for(self.bulk_in_ep);
        let (submitted, res) = crate::xhci::with_controller(self.controller, |ctrl| {
            match ctrl.submit_bulk_transfer(self.slot, ep_idx, phys, CSW_LEN as u32) {
                Err(e) => (false, Err(e)),
                Ok(_) => (true, ctrl.wait_for_transfer(5_000_000)),
            }
        })
        .unwrap_or((false, Err(crate::xhci::XhciError::NotReady)));
        let csw = if res.is_ok() {
            let mut buf = [0u8; CSW_LEN];
            unsafe {
                core::ptr::copy_nonoverlapping(virt as *const u8, buf.as_mut_ptr(), CSW_LEN);
            }
            let c = Csw::from_bytes(&buf);
            if c.is_valid(expected_tag) {
                Ok(c)
            } else {
                Err("usb-msc: CSW signature/tag mismatch")
            }
        } else {
            Err("usb-msc: recv_csw failed")
        };
        release_dma_page(frame, phys, self.slot, submitted, res.is_ok());
        csw
    }

    // ── Public SCSI commands ─────────────────────────────────────────────────

    /// SCSI INQUIRY (6-byte CDB) — returns the standard 36-byte response.
    ///
    /// Response layout (SCSI-2 §8.2.5):
    ///   [0]      device type (0x00 = direct-access block device)
    ///   [1]      removable media bit (bit 7)
    ///   [2]      SCSI version
    ///   [3]      response data format
    ///   [4]      additional length
    ///   [8..15]  vendor ID (8 bytes, ASCII, space-padded)
    ///   [16..31] product ID (16 bytes, ASCII, space-padded)
    ///   [32..35] product revision level (4 bytes, ASCII)
    pub fn inquiry(&mut self) -> Result<[u8; 36], &'static str> {
        let cdb: [u8; 6] = [SCSI_INQUIRY, 0x00, 0x00, 0x00, 36, 0x00];
        let tag = self.next_tag();
        let cbw = Cbw::new(tag, 36, CBW_FLAG_DATA_IN, 0, &cdb);

        self.send_cbw(&cbw)?;

        let mut buf = [0u8; 36];
        self.recv_bulk_in(&mut buf, 36)?;

        let _csw = self.recv_csw(tag)?;

        Ok(buf)
    }

    /// SCSI READ_CAPACITY(10) — returns `(block_count, block_size)`.
    ///
    /// CDB layout (10 bytes): 0x25, 0x00, LBA[4] (ignored, must be 0),
    /// 0x00, 0x00, PMI=0.  Response is 8 bytes:
    ///   [0..3] last LBA (big-endian u32)
    ///   [4..7] block length in bytes (big-endian u32)
    pub fn read_capacity(&mut self) -> Result<(u64, u32), &'static str> {
        let cdb: [u8; 10] = [
            SCSI_READ_CAPACITY,
            0x00,
            0x00,
            0x00,
            0x00,
            0x00,
            0x00,
            0x00,
            0x00,
            0x00,
        ];
        let tag = self.next_tag();
        let cbw = Cbw::new(tag, 8, CBW_FLAG_DATA_IN, 0, &cdb);

        self.send_cbw(&cbw)?;

        let mut buf = [0u8; 8];
        self.recv_bulk_in(&mut buf, 8)?;

        let _csw = self.recv_csw(tag)?;

        let last_lba = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as u64;
        let block_size = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
        // last_lba is the *last valid* LBA, so total count = last_lba + 1.
        Ok((last_lba.saturating_add(1), block_size))
    }

    /// SCSI READ(10) — read `count` blocks starting at `lba` into `buf`.
    ///
    /// CDB layout (10 bytes):
    ///   [0]     0x28 (READ(10))
    ///   [1]     0x00 (flags / DPO / FUA)
    ///   [2..5]  LBA big-endian u32
    ///   [6]     0x00 (group number)
    ///   [7..8]  transfer length (sectors) big-endian u16
    ///   [9]     0x00 (control)
    pub fn cbw_read_sectors(
        &mut self,
        lba: u64,
        count: u16,
        buf: &mut [u8],
    ) -> Result<(), &'static str> {
        if lba > u32::MAX as u64 {
            // READ(10) CDB uses a 32-bit LBA field; use READ(16) for large disks.
            // MasterChecklist Phase 3.4 — READ(16) not yet implemented.
            return Err("usb-msc: lba > 2^32-1, READ(16) not yet supported");
        }
        let lba32 = lba as u32;
        let data_len = (count as u32) * self.block_size;
        let cdb: [u8; 10] = [
            SCSI_READ_10,
            0x00,
            (lba32 >> 24) as u8,
            (lba32 >> 16) as u8,
            (lba32 >> 8) as u8,
            lba32 as u8,
            0x00,
            (count >> 8) as u8,
            count as u8,
            0x00,
        ];
        let tag = self.next_tag();
        let cbw = Cbw::new(tag, data_len, CBW_FLAG_DATA_IN, 0, &cdb);

        self.send_cbw(&cbw)?;
        self.recv_bulk_in(buf, data_len as usize)?;
        let csw = self.recv_csw(tag)?;

        match csw.status {
            CSW_STATUS_GOOD => Ok(()),
            CSW_STATUS_FAILED => Err("usb-msc: SCSI command failed (CHECK CONDITION)"),
            _ => Err("usb-msc: CSW phase error"),
        }
    }

    /// SCSI WRITE(10) — write `buf` (must be a multiple of `block_size`) to `lba`.
    ///
    /// CDB layout mirrors READ(10) with opcode 0x2A.
    pub fn cbw_write_sectors(
        &mut self,
        lba: u64,
        count: u16,
        buf: &[u8],
    ) -> Result<(), &'static str> {
        if lba > u32::MAX as u64 {
            return Err("usb-msc: lba > 2^32-1, WRITE(16) not yet supported");
        }
        let lba32 = lba as u32;
        let data_len = (count as u32) * self.block_size;
        let cdb: [u8; 10] = [
            SCSI_WRITE_10,
            0x00,
            (lba32 >> 24) as u8,
            (lba32 >> 16) as u8,
            (lba32 >> 8) as u8,
            lba32 as u8,
            0x00,
            (count >> 8) as u8,
            count as u8,
            0x00,
        ];
        let tag = self.next_tag();
        let cbw = Cbw::new(tag, data_len, CBW_FLAG_DATA_OUT, 0, &cdb);

        self.send_cbw(&cbw)?;

        // Data OUT phase
        let (phys, virt, frame) = alloc_dma_page()?;
        let ep_idx = ep_index_for(self.bulk_out_ep);

        let mut offset = 0;
        let mut success = true;

        // Write the data in chunks if needed
        while offset < data_len as usize {
            let chunk = (data_len as usize - offset).min(4096);
            unsafe {
                core::ptr::copy_nonoverlapping(buf[offset..].as_ptr(), virt as *mut u8, chunk);
            }
            let res = crate::xhci::with_controller(self.controller, |ctrl| {
                let r1 = ctrl.submit_bulk_transfer(self.slot, ep_idx, phys, chunk as u32);
                if let Err(e) = r1 {
                    Err(e)
                } else {
                    ctrl.wait_for_transfer(5_000_000)
                }
            })
            .unwrap_or(Err(crate::xhci::XhciError::NotReady));
            if res.is_err() {
                success = false;
                break;
            }
            offset += chunk;
        }

        free_dma_page(frame);

        if !success {
            return Err("usb-msc: Data OUT phase failed");
        }

        let csw = self.recv_csw(tag)?;
        match csw.status {
            CSW_STATUS_GOOD => Ok(()),
            _ => Err("usb-msc: SCSI write failed"),
        }
    }

    /// SCSI SYNCHRONIZE CACHE (10) — forces the drive to flush its volatile write cache to flash media.
    pub fn cbw_synchronize_cache(&mut self) -> Result<(), &'static str> {
        let cdb: [u8; 10] = [
            0x35, // SYNCHRONIZE CACHE (10)
            0x00, 0x00, 0x00, 0x00, 0x00, // LBA (ignored/entire disk)
            0x00, // group number
            0x00, 0x00, // number of blocks (0 = all blocks)
            0x00, // control
        ];
        let tag = self.next_tag();
        let cbw = Cbw::new(tag, 0, CBW_FLAG_DATA_OUT, 0, &cdb);

        self.send_cbw(&cbw)?;
        let csw = self.recv_csw(tag)?;
        match csw.status {
            CSW_STATUS_GOOD => Ok(()),
            _ => Err("usb-msc: SCSI synchronize cache failed"),
        }
    }
}

// ─── BlockDevice impl ────────────────────────────────────────────────────────

impl BlockDevice for UsbMscDevice {
    /// Read one 512-byte sector at `lba` into `buf`.
    ///
    /// `buf` must be at least `sector_size()` bytes long.
    fn read_sector(&self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        if buf.len() < self.block_size as usize {
            return Err("usb-msc: buffer too small for read_sector");
        }
        if lba >= self.block_count {
            return Err("usb-msc: read_sector LBA out of range");
        }

        let mut devs = MSC_DEVICES.lock();
        let mut me = None;
        for dev in devs.iter_mut() {
            // Slots are per-controller — two controllers can both have a
            // slot 1, so the controller index is part of the identity.
            if dev.controller == self.controller
                && dev.slot == self.slot
                && dev.lun_count == self.lun_count
            {
                me = Some(dev);
                break;
            }
        }
        let me = me.ok_or("usb-msc: device not found")?;
        me.cbw_read_sectors(lba, 1, buf)?;
        Ok(())
    }

    /// Write one sector.  MSC devices are removable; we conservatively
    /// allow writes so that VFAT / exFAT drivers can mount them rw.
    fn write_sector(&self, lba: u64, buf: &[u8]) -> Result<(), &'static str> {
        // SAFETY GATE (must be FIRST): in `--safe` builds refuse every write so a
        // bare-metal smoke boot from a USB stick cannot clobber it (or any disk).
        // A USB stick is a real disk — without this it was the one BlockDevice
        // that bypassed safe mode (NVMe/AHCI/virtio all guard here).
        crate::block_io::safe_mode_guard_write(lba, buf.len(), "usb-msc")?;
        if buf.len() < self.block_size as usize {
            return Err("usb-msc: buffer too small for write_sector");
        }
        if lba >= self.block_count {
            return Err("usb-msc: write_sector LBA out of range");
        }

        let mut devs = MSC_DEVICES.lock();
        let mut me = None;
        for dev in devs.iter_mut() {
            // Controller index is part of the identity (see read_sector).
            if dev.controller == self.controller
                && dev.slot == self.slot
                && dev.lun_count == self.lun_count
            {
                me = Some(dev);
                break;
            }
        }
        let me = me.ok_or("usb-msc: device not found")?;
        me.cbw_write_sectors(lba, 1, buf)?;
        Ok(())
    }

    fn sector_size(&self) -> usize {
        self.block_size as usize
    }

    fn total_sectors(&self) -> u64 {
        self.block_count
    }

    fn flush_cache(&self) -> Result<(), &'static str> {
        let mut devs = MSC_DEVICES.lock();
        let mut me = None;
        for dev in devs.iter_mut() {
            if dev.slot == self.slot && dev.lun_count == self.lun_count {
                me = Some(dev);
                break;
            }
        }
        let me = me.ok_or("usb-msc: device not found")?;
        me.cbw_synchronize_cache()
    }
}

// ─── Probe (enumerate USB slots for MSC interface) ───────────────────────────

/// Iterate xHCI device slots 1..=max_slots and return all that expose a
/// Mass Storage class interface (class=0x08, subclass=0x06, protocol=0x50).
///
/// On QEMU without an attached USB storage device this will always return
/// an empty Vec, which is the expected boot-time state.
///
/// ## How probing works on real hardware
///
/// 1. Read the Configuration Descriptor (type=0x02) for each active slot.
/// 2. Walk the descriptor chain looking for Interface Descriptors
///    (bDescriptorType=0x04).
/// 3. For each MSC interface, find the two Bulk endpoints (IN and OUT).
/// 4. Send GET_MAX_LUN (class-specific control request, bRequest=0xFE) to
///    learn the number of LUNs.
/// 5. For each LUN send READ_CAPACITY(10) to populate `block_count` /
///    `block_size`.
///
/// All of this requires a live xHCI device; the current implementation
/// returns an empty Vec gracefully so boot proceeds.
pub fn probe_all_msc() -> Vec<UsbMscDevice> {
    let mut found: Vec<UsbMscDevice> = Vec::new();
    // Walk EVERY bound controller — slots are per-controller, and the boot
    // USB stick may sit on a secondary (Athena: 4 xHCI port groups).
    for ctrl_idx in 0..crate::xhci::controller_count() {
        probe_controller_msc(ctrl_idx, &mut found);
    }
    found
}

fn probe_controller_msc(ctrl_idx: usize, found: &mut Vec<UsbMscDevice>) {
    let slot_count =
        crate::xhci::with_controller(ctrl_idx, |ctrl| ctrl.active_slot_count() as u8).unwrap_or(0);

    for slot in 1..=slot_count {
        // Read the device's Configuration Descriptor to identify the interface
        // class.  This uses the xHCI GET_DESCRIPTOR control transfer.
        //
        // In a live environment `get_descriptor(slot, 0x02, 0, 255)` returns
        // the full configuration descriptor chain; we parse the embedded
        // Interface Descriptors here.
        let config_desc =
            crate::xhci::with_controller(ctrl_idx, |ctrl| ctrl.get_descriptor(slot, 0x02, 0, 255));

        let config_bytes = match config_desc {
            Some(Ok(b)) => b,
            _ => continue, // slot unreachable (not enumerated yet)
        };

        // Walk descriptors looking for Interface (type 0x04).
        let mut offset = 0usize;
        while offset + 2 <= config_bytes.len() {
            let desc_len = config_bytes[offset] as usize;
            let desc_type = config_bytes[offset + 1];

            if desc_len < 2 || offset + desc_len > config_bytes.len() {
                break;
            }

            // bDescriptorType == 0x04 → Interface Descriptor (9 bytes min).
            if desc_type == 0x04 && desc_len >= 9 {
                let class = config_bytes[offset + 5];
                let subclass = config_bytes[offset + 6];
                let protocol = config_bytes[offset + 7];

                if class == USB_CLASS_MSC
                    && subclass == USB_SUBCLASS_SCSI
                    && protocol == USB_PROTO_BOT
                {
                    // Found an MSC BOT interface.  Now locate the two Bulk
                    // endpoints in the following Endpoint Descriptors.
                    let mut bulk_in_ep: u8 = 0;
                    let mut bulk_out_ep: u8 = 0;
                    let mut ep_offset = offset + desc_len;

                    while ep_offset + 2 <= config_bytes.len() {
                        let ep_len = config_bytes[ep_offset] as usize;
                        let ep_type = config_bytes[ep_offset + 1];

                        if ep_len < 2 {
                            break;
                        }

                        // bDescriptorType == 0x05 → Endpoint Descriptor (7 bytes).
                        if ep_type == 0x05 && ep_len >= 7 {
                            let ep_addr = config_bytes[ep_offset + 2];
                            let ep_attr = config_bytes[ep_offset + 3];
                            let is_bulk = (ep_attr & 0x03) == 0x02;
                            let is_in = (ep_addr & 0x80) != 0;

                            if is_bulk {
                                if is_in {
                                    bulk_in_ep = ep_addr;
                                } else {
                                    bulk_out_ep = ep_addr;
                                }
                            }
                        } else if ep_type == 0x04 {
                            // Hit the next interface descriptor; stop.
                            break;
                        }

                        ep_offset += ep_len;
                    }

                    if bulk_in_ep == 0 || bulk_out_ep == 0 {
                        // Incomplete endpoint set; skip.
                        offset += desc_len;
                        continue;
                    }

                    // GET_MAX_LUN (class request 0xFE) is OPTIONAL — many devices
                    // STALL it, and issuing a data-IN control transfer with no
                    // backing buffer made QEMU's usb-storage abort the VM
                    // (`usb_packet_copy` assert: device returns 1 byte into a
                    // zero-size IOV). We don't support multi-LUN yet, so default
                    // to a single LUN without the fragile probe.
                    let lun_count: u8 = 0; // 0 = max index 0 = 1 LUN

                    let mut dev = UsbMscDevice {
                        controller: ctrl_idx,
                        slot,
                        lun_count: lun_count.saturating_add(1),
                        bulk_in_ep,
                        bulk_out_ep,
                        block_count: 0,
                        block_size: 512,
                        tag: 0,
                    };

                    // Populate geometry via READ_CAPACITY(10).
                    if let Ok((blocks, bsz)) = dev.read_capacity() {
                        dev.block_count = blocks;
                        dev.block_size = bsz;
                        crate::serial_println!(
                            "[usb-msc] slot={} lun_count={} bulk_in={:#x} bulk_out={:#x} \
                             blocks={} bsize={}",
                            slot,
                            dev.lun_count,
                            bulk_in_ep,
                            bulk_out_ep,
                            blocks,
                            bsz
                        );
                    }

                    found.push(dev);
                }
            }

            offset += desc_len;
        }
    }
}

// ─── Global Device List ──────────────────────────────────────────────────────

/// All probed MSC devices.  Populated by `init()` and accessible to the
/// block layer, partition scanner, and VFS mount code.
pub static MSC_DEVICES: Mutex<Vec<UsbMscDevice>> = Mutex::new(Vec::new());

/// Build a fresh `BlockDevice` handle for each probed MSC device. The handles
/// key into `MSC_DEVICES` by `(slot, lun_count)` for the actual transfer, so
/// they can be used independently of `ACTIVE_BLOCK_DEVICE` — e.g. the bootlog
/// persistence path probes USB drives for `BOOTLOG.TXT` without disturbing the
/// active (NVMe) disk.
pub fn msc_block_devices() -> Vec<alloc::boxed::Box<dyn BlockDevice>> {
    let devs = MSC_DEVICES.lock();
    devs.iter()
        .map(|d| {
            alloc::boxed::Box::new(UsbMscDevice {
                controller: d.controller,
                slot: d.slot,
                lun_count: d.lun_count,
                bulk_in_ep: d.bulk_in_ep,
                bulk_out_ep: d.bulk_out_ep,
                block_count: d.block_count,
                block_size: d.block_size,
                tag: 0,
            }) as alloc::boxed::Box<dyn BlockDevice>
        })
        .collect()
}

// ─── R10 Contract ────────────────────────────────────────────────────────────

/// Enumerate USB MSC devices and register them with the block layer.
///
/// Called from `kernel_main` after xHCI has been initialised.
/// Re-run the MSC probe iff the registry is empty — used by the bootlog
/// end-of-boot retry, in case a slot finished enumerating after `init()`.
pub fn reprobe_if_empty() {
    if !MSC_DEVICES.lock().is_empty() {
        return;
    }
    let devices = probe_all_msc();
    if !devices.is_empty() {
        crate::serial_println!(
            "[usb-msc] late reprobe found {} MSC device(s)",
            devices.len()
        );
        *MSC_DEVICES.lock() = devices;
    }
}

pub fn init() {
    crate::serial_println!("[usb-msc] probing USB Mass Storage devices...");
    let devices = probe_all_msc();
    let count = devices.len();
    *MSC_DEVICES.lock() = devices;

    if count == 0 {
        crate::serial_println!(
            "[usb-msc] no MSC devices found — no usb-storage attached, or the stick's port/hub failed enumeration (check [xhci] lines above)"
        );
    } else {
        crate::serial_println!("[usb-msc] found {} MSC device(s)", count);
        // Register each device with the block layer so partition scanning can
        // find them.  Major 15 = USB storage (sd-style numbering starts at 8
        // in block_io; we use 15 to avoid collisions with NVMe/AHCI).
        let mut bl = crate::block_io::BLOCK_LAYER.lock();
        if let Some(layer) = bl.as_mut() {
            let devs = MSC_DEVICES.lock();
            for (i, dev) in devs.iter().enumerate() {
                let mut info =
                    crate::block_io::BlockDeviceInfo::new(format!("usb{}", i), 15, i as u16);
                info.total_sectors = dev.block_count;
                layer.register_device(info);

                let has_active = crate::block_io::ACTIVE_BLOCK_DEVICE.lock().is_some();
                if !has_active {
                    crate::block_io::set_active_block_device(alloc::boxed::Box::new(
                        UsbMscDevice {
                            controller: dev.controller,
                            slot: dev.slot,
                            lun_count: dev.lun_count,
                            bulk_in_ep: dev.bulk_in_ep,
                            bulk_out_ep: dev.bulk_out_ep,
                            block_count: dev.block_count,
                            block_size: dev.block_size,
                            tag: 0,
                        },
                    ));
                    crate::serial_println!("[usb-msc] registered as active block device");
                }
            }
        }
    }
}

/// Boot smoketest — always passes on QEMU because zero MSC devices is the
/// expected state.  On real hardware with an attached drive the count > 0
/// branch is also a PASS.
pub fn run_boot_smoketest() {
    // Snapshot device geometry (don't hold the lock across the block read).
    let dev0 = {
        let devs = MSC_DEVICES.lock();
        devs.first().map(|d| {
            (
                d.controller,
                d.slot,
                d.lun_count,
                d.block_count,
                d.block_size,
            )
        })
    };

    let Some((controller, slot, lun_count, block_count, block_size)) = dev0 else {
        crate::serial_println!(
            "[usb-msc] smoketest: msc_devices=0 -> PASS (no MSC device on QEMU)"
        );
        return;
    };

    // Read sector 0 through the BlockDevice path (full CBW→data→CSW round-trip)
    // and verify the QEMU backing image's signature landed — proves enumeration
    // + bulk endpoints + SCSI READ(10) actually work end to end.
    let probe = UsbMscDevice {
        controller,
        slot,
        lun_count,
        bulk_in_ep: 0,
        bulk_out_ep: 0,
        block_count,
        block_size,
        tag: 0,
    };
    let mut sec = alloc::vec![0u8; block_size as usize];
    let read_ok = probe.read_sector(0, &mut sec).is_ok();
    let sig = b"ATHENAOS-USB-MSC-SECTOR0";
    let sig_ok = read_ok && sec.len() >= sig.len() && &sec[..sig.len()] == sig;

    // Non-destructive write+readback on a scratch sector (the last block):
    // save → write pattern → read back + verify → restore. Proves WRITE(10)
    // works, which the bootlog-on-USB path depends on.
    let scratch_lba = block_count.saturating_sub(1);
    let mut write_ok = false;
    if read_ok && scratch_lba > 0 {
        let mut original = alloc::vec![0u8; block_size as usize];
        if probe.read_sector(scratch_lba, &mut original).is_ok() {
            let mut pattern = alloc::vec![0u8; block_size as usize];
            for (i, b) in pattern.iter_mut().enumerate() {
                *b = (i as u8) ^ 0x5A;
            }
            if probe.write_sector(scratch_lba, &pattern).is_ok() {
                let mut back = alloc::vec![0u8; block_size as usize];
                write_ok = probe.read_sector(scratch_lba, &mut back).is_ok() && back == pattern;
                // Restore the original contents.
                let _ = probe.write_sector(scratch_lba, &original);
            }
        }
    }

    let pass = read_ok && sig_ok && write_ok;
    crate::serial_println!(
        "[usb-msc] smoketest: msc_devices=1 slot={} blocks={} bsize={} read0_ok={} sig_ok={} write_rt_ok={} -> {}",
        slot,
        block_count,
        block_size,
        read_ok,
        sig_ok,
        write_ok,
        if pass { "PASS" } else { "FAIL" }
    );
}

/// Human-readable status dump for `/proc/athena/usb_msc` or the boot log.
pub fn dump_text() -> String {
    let devs = MSC_DEVICES.lock();
    if devs.is_empty() {
        return "usb-msc: no devices detected\n".to_string();
    }
    let mut out = String::new();
    for (i, d) in devs.iter().enumerate() {
        let capacity_mb = (d.block_count * d.block_size as u64) / (1024 * 1024);
        out.push_str(&format!(
            "usb{}: slot={} luns={} bulk_in={:#x} bulk_out={:#x} \
             blocks={} block_size={} capacity={}MB\n",
            i,
            d.slot,
            d.lun_count,
            d.bulk_in_ep,
            d.bulk_out_ep,
            d.block_count,
            d.block_size,
            capacity_mb
        ));
    }
    out
}
