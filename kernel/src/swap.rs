//! Swap area — page-out / page-in over a block device (Concept §Memory: the
//! machine never dies under pressure; cold anonymous pages spill to disk and
//! fault back transparently). MasterChecklist Phase 4.1 — "Swap to disk".
//!
//! A [`SwapArea`] divides a backing block device into 4 KiB slots (8 ×
//! 512-byte sectors each), tracked by a bitmap allocator. [`swap_out`]
//! copies a physical frame's bytes into a free slot and returns its index;
//! [`swap_in`] copies a slot back into a frame; [`free_slot`] releases it.
//! That is the storage half of swap. The reclaim policy (choosing victim
//! anon pages) and the page-fault swap-in handler that makes it transparent
//! are the wire-up follow-up — this module is the proven mechanism they
//! call.
//!
//! The smoketest is a deterministic round trip over a RAM-backed device:
//! a frame is filled with a pattern, paged OUT, the frame zeroed, paged
//! back IN, and the pattern verified — plus slot allocator exhaustion +
//! free/reuse. Identical on QEMU and iron (no real disk needed for the
//! proof; the production path points `SwapArea` at a swap partition).

#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use crate::block_io::BlockDevice;

/// 4 KiB page = 8 × 512-byte sectors.
pub const PAGE_SIZE: usize = 4096;
const SECTORS_PER_PAGE: u64 = 8;

/// A swap slot index (page granularity within the area).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapSlot(pub u64);

pub struct SwapArea {
    dev: Box<dyn BlockDevice>,
    /// Slot occupancy bitmap, one bit per page-slot.
    bitmap: Vec<u64>,
    total_slots: u64,
    used_slots: u64,
}

impl SwapArea {
    /// Build a swap area over `dev`. Slot 0 is reserved (a 0 slot index can
    /// double as "not swapped" in PTE-stored swap entries later), so usable
    /// slots run 1..total.
    pub fn new(dev: Box<dyn BlockDevice>) -> Self {
        let total_slots = dev.total_sectors() / SECTORS_PER_PAGE;
        let words = ((total_slots + 63) / 64) as usize;
        let mut bitmap = alloc::vec![0u64; words.max(1)];
        // Reserve slot 0.
        if total_slots > 0 {
            bitmap[0] |= 1;
        }
        Self {
            dev,
            bitmap,
            total_slots,
            used_slots: if total_slots > 0 { 1 } else { 0 },
        }
    }

    fn test(&self, slot: u64) -> bool {
        self.bitmap[(slot / 64) as usize] & (1 << (slot % 64)) != 0
    }
    fn set(&mut self, slot: u64) {
        self.bitmap[(slot / 64) as usize] |= 1 << (slot % 64);
    }
    fn clear(&mut self, slot: u64) {
        self.bitmap[(slot / 64) as usize] &= !(1 << (slot % 64));
    }

    fn alloc_slot(&mut self) -> Option<SwapSlot> {
        for s in 1..self.total_slots {
            if !self.test(s) {
                self.set(s);
                self.used_slots += 1;
                return Some(SwapSlot(s));
            }
        }
        None
    }

    /// Copy a 4 KiB page out of `page` into a freshly allocated slot.
    /// `page` must be exactly [`PAGE_SIZE`] bytes.
    pub fn swap_out(&mut self, page: &[u8]) -> Result<SwapSlot, &'static str> {
        if page.len() != PAGE_SIZE {
            return Err("swap_out: page must be 4096 bytes");
        }
        let slot = self.alloc_slot().ok_or("swap_out: swap area full")?;
        let base = slot.0 * SECTORS_PER_PAGE;
        for i in 0..SECTORS_PER_PAGE {
            let off = (i as usize) * 512;
            if self
                .dev
                .write_sector(base + i, &page[off..off + 512])
                .is_err()
            {
                // Roll back the allocation so a write fault doesn't leak a slot.
                self.clear(slot.0);
                self.used_slots -= 1;
                return Err("swap_out: device write failed");
            }
        }
        PAGES_OUT.fetch_add(1, Ordering::Relaxed);
        Ok(slot)
    }

    /// Copy `slot`'s page back into `page` (exactly [`PAGE_SIZE`] bytes).
    /// The slot stays allocated — callers `free_slot` once the page is live
    /// again so a fault that re-reads before completion still finds it.
    pub fn swap_in(&self, slot: SwapSlot, page: &mut [u8]) -> Result<(), &'static str> {
        if page.len() != PAGE_SIZE {
            return Err("swap_in: page must be 4096 bytes");
        }
        if slot.0 == 0 || slot.0 >= self.total_slots || !self.test(slot.0) {
            return Err("swap_in: invalid or free slot");
        }
        let base = slot.0 * SECTORS_PER_PAGE;
        for i in 0..SECTORS_PER_PAGE {
            let off = (i as usize) * 512;
            if self
                .dev
                .read_sector(base + i, &mut page[off..off + 512])
                .is_err()
            {
                return Err("swap_in: device read failed");
            }
        }
        PAGES_IN.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn free_slot(&mut self, slot: SwapSlot) {
        if slot.0 != 0 && slot.0 < self.total_slots && self.test(slot.0) {
            self.clear(slot.0);
            self.used_slots -= 1;
        }
    }

    pub fn total_slots(&self) -> u64 {
        self.total_slots
    }
    pub fn used_slots(&self) -> u64 {
        self.used_slots
    }
    pub fn free_slots(&self) -> u64 {
        self.total_slots.saturating_sub(self.used_slots)
    }
}

/// The live swap area (None until a swap partition is activated).
pub static SWAP: Mutex<Option<SwapArea>> = Mutex::new(None);
static PAGES_OUT: AtomicU64 = AtomicU64::new(0);
static PAGES_IN: AtomicU64 = AtomicU64::new(0);

pub fn init() {
    crate::serial_println!(
        "[swap] page-out/page-in engine ready (4 KiB slots; activate with a swap partition)"
    );
}

/// Deterministic proof over a RAM-backed device: a page round-trips out and
/// back byte-for-byte; the slot allocator exhausts and recycles correctly.
pub fn run_boot_smoketest() {
    // 64-slot RAM swap (64 × 8 = 512 sectors).
    let (disk, _store) = crate::fde::SharedRamDisk::new(512);
    let mut area = SwapArea::new(Box::new(disk));
    let total = area.total_slots();

    // 1. Round trip: fill a page, swap out, zero it, swap in, compare.
    let mut page = alloc::vec![0u8; PAGE_SIZE];
    for (i, b) in page.iter_mut().enumerate() {
        *b = (i.wrapping_mul(31).wrapping_add(7) % 251) as u8;
    }
    let original = page.clone();
    let slot = match area.swap_out(&page) {
        Ok(s) => s,
        Err(e) => {
            crate::serial_println!("[swap] smoketest: swap_out failed ({}) -> FAIL", e);
            return;
        }
    };
    page.iter_mut().for_each(|b| *b = 0);
    let zeroed = page.iter().all(|&b| b == 0);
    let in_ok = area.swap_in(slot, &mut page).is_ok();
    let roundtrip = in_ok && page == original;

    // 2. A second page gets a DISTINCT slot.
    let slot2 = area.swap_out(&original).ok();
    let distinct = slot2.map(|s| s != slot).unwrap_or(false);

    // 3. Free + reuse: releasing slot lets a later alloc reclaim it.
    area.free_slot(slot);
    let used_after_free = area.used_slots();
    let slot3 = area.swap_out(&original).ok();
    let reused = slot3 == Some(slot);

    // 4. Exhaustion: fill every remaining slot, the next swap_out fails
    // cleanly (no slot leak, used == total).
    while area.swap_out(&original).is_ok() {}
    let exhausted = area.free_slots() == 0 && area.used_slots() == total;
    let overflow_clean = area.swap_out(&original).is_err();

    let pass = zeroed && roundtrip && distinct && reused && exhausted && overflow_clean;
    crate::serial_println!(
        "[swap] smoketest: roundtrip={} distinct_slot={} free_reuse={}(used_after_free={}) exhaust={} overflow_refused={} -> {}",
        roundtrip,
        distinct,
        reused,
        used_after_free,
        exhausted,
        overflow_clean,
        if pass { "PASS" } else { "FAIL" },
    );
}

/// `/proc/raeen/swap` — swap area state.
pub fn dump_text() -> String {
    let guard = SWAP.lock();
    let (total, used) = guard
        .as_ref()
        .map(|a| (a.total_slots(), a.used_slots()))
        .unwrap_or((0, 0));
    alloc::format!(
        "# swap (page-out/page-in, 4 KiB slots)\nactive: {}\ntotal_slots: {}\nused_slots: {}\npages_out: {}\npages_in: {}\n",
        guard.is_some(),
        total,
        used,
        PAGES_OUT.load(Ordering::Relaxed),
        PAGES_IN.load(Ordering::Relaxed),
    )
}
