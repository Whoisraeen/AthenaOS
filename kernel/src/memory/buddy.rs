//! Buddy Allocator for physical frames.
//!
//! Concept §Memory: "needs buddy upgrade (10 MiB allocs in < 50 µs)".
//!
//! This implementation uses doubly linked lists stored in free frames
//! to manage blocks of orders 0 to 11 (4 KiB to 8 MiB).
//! A bitmap tracks the free/used status of each page for merging.

use crate::arch::PhysAddr;
use crate::memory::phys_to_virt;

pub const MAX_ORDER: usize = 12; // 0..11 -> 2^11 * 4KB = 8 MiB block

pub struct BuddyAllocator {
    /// Heads of doubly linked lists for each order.
    free_lists: [u64; MAX_ORDER],
    /// Bitmap of free status (1 = used, 0 = free).
    bitmap: &'static mut [u64],
    total_frames: usize,
    free_frames: usize,
    pub node_id: u32,
}

impl BuddyAllocator {
    /// Create a new buddy allocator using a provided bitmap for a specific NUMA node.
    /// # Safety
    /// `bitmap_ptr` must be valid for `bitmap_len` u64s.
    pub unsafe fn new(
        bitmap_ptr: *mut u64,
        bitmap_len: usize,
        total_frames: usize,
        node_id: u32,
    ) -> Self {
        let bitmap = core::slice::from_raw_parts_mut(bitmap_ptr, bitmap_len);
        // Mark everything as USED initially.
        for word in bitmap.iter_mut() {
            *word = !0;
        }

        Self {
            free_lists: [0; MAX_ORDER],
            bitmap,
            total_frames,
            free_frames: 0,
            node_id,
        }
    }

    /// Add a range of physical memory to the allocator.
    pub unsafe fn add_range(&mut self, start: u64, end: u64) {
        let mut addr = (start + 4095) & !4095;
        let end = end & !4095;
        let max_addr = (self.total_frames as u64).saturating_mul(4096);

        while addr < end {
            if addr >= max_addr {
                break;
            }
            // Find largest power-of-two block that is aligned and fits.
            let mut order = 0;
            while order < MAX_ORDER - 1 {
                let size = 1u64 << (order + 1 + 12);
                if addr % size != 0 || addr + size > end {
                    break;
                }
                order += 1;
            }
            self.free_block(addr, order as u8);
            addr += 1u64 << (order + 12);
        }
    }

    /// Allocate a block of frames of the given order (2^order pages).
    pub unsafe fn alloc_block(&mut self, order: u8) -> Option<u64> {
        let order = order as usize;
        if order >= MAX_ORDER {
            return None;
        }

        for o in order..MAX_ORDER {
            if self.free_lists[o] != 0 {
                let block = self.free_lists[o];
                self.pop_from_list(block, o as u8);

                // Split larger block down to the requested order.
                let mut current_o = o;
                while current_o > order {
                    current_o -= 1;
                    let buddy = block + (1u64 << (current_o + 12));
                    self.push_to_list(buddy, current_o as u8);
                    // Mark buddy range as free in bitmap (it was part of a larger free block)
                    // Actually, when we split, the base 'block' stays 'free' until the end.
                }

                // Mark the final allocated range as USED.
                self.mark_range(block, 1 << order, true);
                self.free_frames -= 1 << order;
                return Some(block);
            }
        }
        None
    }

    /// Free a block of frames.
    ///
    /// Double-free guard: the per-frame bitmap is the authority for
    /// allocated vs. free. If the base frame of `addr` is already marked
    /// free, this is a double-free — pushing it again would link the same
    /// node onto the free list twice, so a later `alloc_block` hands the
    /// same physical frame out twice (UAF / aliasing). Reject it (no
    /// double-count of `free_frames`, no second `push_to_list`) and WARN.
    pub unsafe fn free_block(&mut self, mut addr: u64, mut order: u8) {
        if self.is_block_free(addr, order) {
            crate::serial_println!(
                "[buddy] WARN double-free rejected: addr={:#x} order={}",
                addr,
                order
            );
            return;
        }

        self.free_frames += 1 << order;

        while (order as usize) < MAX_ORDER - 1 {
            let buddy = addr ^ (1u64 << (order + 12));
            if self.block_on_free_list(buddy, order) {
                // Found a real free buddy of the same order! Merge.
                self.pop_from_list(buddy, order);
                addr = addr.min(buddy);
                order += 1;
            } else {
                break;
            }
        }

        self.push_to_list(addr, order);
        self.mark_range(addr, 1 << order, false);
    }

    /// True only if EVERY frame in the `2^order` block at `addr` is marked free.
    ///
    /// Checking only the base frame (the old `let _ = order;` behaviour) let a
    /// merge in `free_block` swallow a block whose base was free but which still
    /// CONTAINED a live allocation (e.g. a kernel-stack page table carved out of
    /// the range): `block_on_free_list` accepted it, the merge built a larger
    /// "free" block over the used frame, and a later split handed that used frame
    /// back out — the kstack-PT double-allocation that corrupted kernel stacks
    /// during ELF spawn (deterministic under KVM's allocation order). Scanning
    /// the whole block makes "is this block free" honest, so only a genuinely
    /// free buddy is ever merged.
    fn is_block_free(&self, addr: u64, order: u8) -> bool {
        let base = (addr >> 12) as usize;
        let count = 1usize << (order as usize);
        for i in 0..count {
            let frame_idx = base + i;
            if frame_idx >= self.total_frames {
                return false;
            }
            let word = frame_idx / 64;
            let bit = frame_idx % 64;
            if word >= self.bitmap.len() {
                return false;
            }
            if (self.bitmap[word] & (1u64 << bit)) != 0 {
                return false; // any used frame in the range → block is NOT free
            }
        }
        true
    }

    /// True only if `addr` is genuinely a free block of exactly `order` that is
    /// currently linked on `free_lists[order]`.
    ///
    /// `is_block_free` only inspects the per-frame bitmap and *cannot* tell a
    /// standalone order-`order` free block apart from a frame that merely lies
    /// inside a larger free block (or whose memory still holds stale data such
    /// as page-table entries). Relying on it for buddy merges let
    /// `pop_from_list` read those stale bytes as a list pointer and feed a
    /// bogus physical address into `phys_to_virt`, panicking with a
    /// non-canonical VirtAddr during user page-table teardown. This validates
    /// the in-block free-list node and its back-linkage in O(1) so only a real
    /// free buddy is ever merged.
    unsafe fn block_on_free_list(&self, addr: u64, order: u8) -> bool {
        if !self.is_block_free(addr, order) {
            return false;
        }
        let max_addr = (self.total_frames as u64).saturating_mul(4096);
        let valid = |p: u64| p == 0 || (p < max_addr && (p & 0xFFF) == 0);
        let virt = phys_to_virt(addr).as_ptr::<u64>();
        let next = *virt;
        let prev = *virt.add(1);
        if !valid(next) || !valid(prev) {
            return false;
        }
        if prev == 0 {
            self.free_lists[order as usize] == addr
        } else {
            // prev.next must link back to addr for a consistent free list.
            let prev_next = *phys_to_virt(prev).as_ptr::<u64>();
            prev_next == addr
        }
    }

    fn mark_range(&mut self, addr: u64, count: usize, used: bool) {
        let start_idx = (addr >> 12) as usize;
        for i in 0..count {
            let idx = start_idx + i;
            let word = idx / 64;
            let bit = idx % 64;
            if word < self.bitmap.len() {
                if used {
                    self.bitmap[word] |= 1 << bit;
                } else {
                    self.bitmap[word] &= !(1 << bit);
                }
            }
        }
    }

    unsafe fn push_to_list(&mut self, addr: u64, order: u8) {
        let o = order as usize;
        let next = self.free_lists[o];

        // Node: [0..8] = next_phys, [8..16] = prev_phys
        let virt = phys_to_virt(addr).as_mut_ptr::<u64>();
        *virt = next;
        *virt.add(1) = 0; // prev = 0 (head)

        if next != 0 {
            let next_virt = phys_to_virt(next).as_mut_ptr::<u64>();
            *next_virt.add(1) = addr; // next.prev = addr
        }
        self.free_lists[o] = addr;
    }

    unsafe fn pop_from_list(&mut self, addr: u64, order: u8) {
        let o = order as usize;
        let virt = phys_to_virt(addr).as_ptr::<u64>();
        let next = *virt;
        let prev = *virt.add(1);

        if prev != 0 {
            let prev_virt = phys_to_virt(prev).as_mut_ptr::<u64>();
            *prev_virt = next;
        } else {
            // This was the head
            self.free_lists[o] = next;
        }

        if next != 0 {
            let next_virt = phys_to_virt(next).as_mut_ptr::<u64>();
            *next_virt.add(1) = prev;
        }
    }

    pub fn stats(&self) -> (usize, usize) {
        (self.total_frames, self.free_frames)
    }

    /// Snapshot the free-list cardinality at each order. Used by
    /// `/proc/athena/buddy` so userspace and the boot dump can see
    /// fragmentation state without locking the allocator for long.
    pub fn order_counts(&self) -> [usize; MAX_ORDER] {
        let mut counts = [0usize; MAX_ORDER];
        let max_addr = (self.total_frames as u64).saturating_mul(4096);
        // A valid free-list node is a page-aligned physical address in range.
        // Without this guard a corrupted `next` pointer (seen as text-like
        // garbage) feeds a bogus address into `phys_to_virt`, panicking with a
        // non-canonical VirtAddr while the /proc/athena/buddy boot dump walks the
        // list. Stop the walk on the first implausible node instead of faulting.
        let valid = |p: u64| p != 0 && p < max_addr && (p & 0xFFF) == 0;
        for o in 0..MAX_ORDER {
            let mut node = self.free_lists[o];
            let mut hops = 0usize;
            // Cap traversal so a corrupted list can't hang the dumper.
            while node != 0 && hops < 1_000_000 {
                if !valid(node) {
                    break;
                }
                counts[o] += 1;
                let virt = phys_to_virt(node).as_ptr::<u64>();
                node = unsafe { *virt };
                hops += 1;
            }
        }
        counts
    }
}

// ── Boot smoketest ─────────────────────────────────────────────────────
//
// Concept §Memory: "needs buddy upgrade (10 MiB allocs in < 50 µs)".
// We can't allocate a contiguous 10 MiB block from a smoketest without
// risking destabilizing the early boot path, but we *can* measure
// alloc/free latency at small orders to confirm the allocator is on
// the hot path and that free returns memory to the freelists.

pub fn run_boot_smoketest() {
    use crate::memory::BUDDY_ALLOCATORS;

    let mut g = BUDDY_ALLOCATORS.lock();
    let buddy = match g.first_mut() {
        Some(b) => b,
        None => {
            crate::serial_println!("[buddy] smoketest SKIP: no allocator initialized");
            return;
        }
    };

    let (total, free_before) = buddy.stats();
    let mib_total = (total * 4) >> 10;
    let mib_free = (free_before * 4) >> 10;

    // Time a small alloc/free round-trip — order 0 = 4 KiB.
    let t0 = unsafe { core::arch::x86_64::_rdtsc() };
    let blk = unsafe { buddy.alloc_block(0) };
    let t1 = unsafe { core::arch::x86_64::_rdtsc() };
    let pass_alloc = blk.is_some();
    if let Some(addr) = blk {
        unsafe {
            buddy.free_block(addr, 0);
        }
    }
    let t2 = unsafe { core::arch::x86_64::_rdtsc() };

    let alloc_cycles = t1.saturating_sub(t0);
    let free_cycles = t2.saturating_sub(t1);

    let (_, free_after) = buddy.stats();
    let pass_free = free_after == free_before;

    crate::serial_println!(
        "[buddy] smoketest: total={} MiB free={} MiB; alloc(4KiB)={} cycles free={} cycles; \
         alloc_ok={} free_returned={}",
        mib_total,
        mib_free,
        alloc_cycles,
        free_cycles,
        pass_alloc,
        pass_free,
    );

    if !pass_alloc || !pass_free {
        crate::serial_println!(
            "[buddy] smoketest FAIL: alloc_ok={} free_returned={}",
            pass_alloc,
            pass_free
        );
    }

    // Double-free guard proof: alloc a block, free it once (free count
    // returns to baseline), then free the SAME block again — the guard must
    // reject the second free so the free-frame count is UNCHANGED. Without
    // the guard the second free double-counts free_frames and double-pushes
    // the node (free-list corruption). FAIL-able: a regression that removes
    // the guard makes free_after_2nd != free_after_1st.
    let dbl_pass = if let Some(addr) = unsafe { buddy.alloc_block(0) } {
        unsafe { buddy.free_block(addr, 0) };
        let (_, free_after_1st) = buddy.stats();
        // Second free of the already-free block must be rejected.
        unsafe { buddy.free_block(addr, 0) };
        let (_, free_after_2nd) = buddy.stats();
        free_after_2nd == free_after_1st
    } else {
        false
    };

    crate::serial_println!(
        "[buddy] double-free-guard smoketest: rejected_second_free={} -> {}",
        dbl_pass,
        if dbl_pass { "PASS" } else { "FAIL" }
    );

    // is_block_free RANGE-check proof (the kernel-stack-PT double-alloc fix): a
    // multi-frame block counts as free ONLY if EVERY frame in it is free. Alloc
    // an order-1 (2-frame) block, free just the FIRST frame, then the order-1
    // block STILL contains a used frame, so is_block_free(base, 1) MUST be false.
    // The old base-only check returned true here, which let a `free_block` merge
    // swallow a live frame (a kernel-stack page table) and hand it back out — the
    // KVM-deterministic spawn double-fault. FAIL-able: reverting the fix flips it.
    let range_pass = if let Some(base) = unsafe { buddy.alloc_block(1) } {
        unsafe { buddy.free_block(base, 0) }; // free only the FIRST of the 2 frames
        let contains_used = !buddy.is_block_free(base, 1); // 2nd frame still used
        unsafe { buddy.free_block(base + 0x1000, 0) }; // clean up (merges back)
        contains_used
    } else {
        false
    };
    crate::serial_println!(
        "[buddy] is_block_free range-check smoketest: partial-block-not-free={} -> {}",
        range_pass,
        if range_pass { "PASS" } else { "FAIL" }
    );
}
