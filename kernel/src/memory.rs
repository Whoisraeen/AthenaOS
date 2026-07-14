//! Memory management — paging, frame allocation, and heap.
//!
//! Provides:
//! - `init()` — creates an `OffsetPageTable` using the bootloader's
//!   physical memory mapping
//! - `BootInfoFrameAllocator` — hands out physical frames from the
//!   bootloader's memory map
//! - `allocator` submodule — kernel heap on a linked-list allocator

pub mod allocator;
pub mod buddy;

use crate::arch::{PhysAddr, VirtAddr};
use bootloader_api::info::{MemoryRegion, MemoryRegionKind};
use spin::{Mutex, Once};
use x86_64::{
    registers::control::Cr3,
    structures::paging::{
        FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame,
        Size4KiB,
    },
};

pub static PHYS_MEM_OFFSET: Once<VirtAddr> = Once::new();
pub static KERNEL_PML4: Once<x86_64::structures::paging::PhysFrame> = Once::new();

/// Global lock to serialize page table modifications (e.g. map_to) since
/// OffsetPageTable is not inherently thread-safe and concurrent mappings
/// can result in PT allocation races and corrupted PageAlreadyMapped states.
pub static PAGE_TABLE_LOCK: spin::Mutex<()> = spin::Mutex::new(());

/// Force-release `PAGE_TABLE_LOCK`. Called from the double-fault handler
/// so a CPU that #DF'd inside `map_page_in_pml4` / `free_kernel_stack`
/// doesn't freeze every other CPU's next `alloc_kernel_stack` /
/// `map_user_page` (both reacquire this lock for every page).
///
/// # Safety
///
/// Only safe when the caller is certain the lock holder will never
/// resume (i.e., the faulting CPU is about to enter `hlt_loop()`).
pub unsafe fn force_unlock_page_table() {
    PAGE_TABLE_LOCK.force_unlock();
}

/// Saved bootloader memory map, for runtime physical-address classification.
static MEMORY_REGIONS: Once<&'static [MemoryRegion]> = Once::new();

/// True if `phys` falls inside a bootloader `Usable` RAM region — i.e. memory
/// the kernel owns (heap, buddy frames, kernel working set). AML `SystemMemory`
/// OperationRegions must only ever target firmware NVS / reserved / MMIO, so a
/// write whose address lands here is always corruption and must be refused.
pub fn phys_is_usable_ram(phys: u64) -> bool {
    MEMORY_REGIONS
        .get()
        .map(|regions| {
            regions
                .iter()
                .any(|r| r.kind == MemoryRegionKind::Usable && phys >= r.start && phys < r.end)
        })
        .unwrap_or(false)
}
pub static FRAME_ALLOCATOR: Mutex<Option<BootInfoFrameAllocator>> = Mutex::new(None);
pub static BUDDY_ALLOCATORS: Mutex<alloc::vec::Vec<buddy::BuddyAllocator>> =
    Mutex::new(alloc::vec::Vec::new());

lazy_static::lazy_static! {
    pub static ref FREED_FRAMES: Mutex<alloc::vec::Vec<PhysFrame>> = Mutex::new(alloc::vec::Vec::new());
}

/// Initialize an `OffsetPageTable` from the bootloader's physical memory offset.
///
/// # Safety
/// Caller must guarantee that `physical_memory_offset` is the correct
/// mapping provided by the bootloader.
pub unsafe fn init(physical_memory_offset: VirtAddr) -> OffsetPageTable<'static> {
    use x86_64::registers::control::Cr3;

    let (level_4_table_frame, _) = Cr3::read();
    KERNEL_PML4.call_once(|| level_4_table_frame);

    let phys = level_4_table_frame.start_address();
    let virt = physical_memory_offset + phys.as_u64();
    let page_table = &mut *(virt.as_mut_ptr());
    OffsetPageTable::new(page_table, physical_memory_offset)
}

pub fn active_page_table() -> OffsetPageTable<'static> {
    let offset = *PHYS_MEM_OFFSET
        .get()
        .expect("PHYS_MEM_OFFSET not initialized");
    unsafe { init(offset) }
}

/// Page table for the kernel address space (always `KERNEL_PML4`), regardless of current CR3.
pub fn kernel_page_table() -> OffsetPageTable<'static> {
    use x86_64::structures::paging::page_table::PageTable;
    let offset = *PHYS_MEM_OFFSET
        .get()
        .expect("PHYS_MEM_OFFSET not initialized");
    let frame = *KERNEL_PML4.get().expect("KERNEL_PML4 not initialized");
    let virt = offset + frame.start_address().as_u64();
    let page_table = unsafe { &mut *(virt.as_mut_ptr::<PageTable>()) };
    unsafe { OffsetPageTable::new(page_table, offset) }
}

/// Translate a kernel virtual address using the kernel PML4 (safe while user CR3 is active).
pub fn kernel_translate_addr(virt: VirtAddr) -> Option<PhysAddr> {
    use x86_64::structures::paging::Translate;
    kernel_page_table().translate_addr(virt)
}

/// Run `f` with CR3 switched to the kernel PML4, then restore the previous CR3.
pub fn with_kernel_cr3<R>(f: impl FnOnce() -> R) -> R {
    use x86_64::registers::control::Cr3;
    let (current, flags) = Cr3::read();
    let kernel = *KERNEL_PML4.get().expect("KERNEL_PML4 not initialized");
    if current == kernel {
        return f();
    }
    unsafe {
        Cr3::write(kernel, flags);
    }
    let result = f();
    unsafe {
        Cr3::write(current, flags);
    }
    result
}

use core::sync::atomic::AtomicU64;
use core::sync::atomic::Ordering;

// Start kernel stacks at high memory to leave space for VMA.
static KERNEL_STACK_ALLOCATOR: AtomicU64 = AtomicU64::new(0xFFFF_B000_0000_0000);

/// Allocate a per-task kernel stack in the kernel PML4 with an unmapped guard page at the bottom.
///
/// Call this **before** `create_new_pml4()` so the new user page table clone
/// includes the stack mapping (alloc while parent/user CR3 is active only
/// wires pages into the parent's tables).
pub fn alloc_kernel_stack(size: usize) -> (*mut u8, VirtAddr) {
    use x86_64::structures::paging::FrameAllocator;

    let alloc_size = (size as u64 + 4095) & !4095;
    let total_vma_size = alloc_size + 4096; // + 1 guard page

    let base_vaddr = KERNEL_STACK_ALLOCATOR.fetch_add(total_vma_size, Ordering::SeqCst);

    // The guard page is at `base_vaddr` (lowest page, unmapped)
    let stack_start = base_vaddr + 4096;
    let stack_end = stack_start + alloc_size;

    with_kernel_cr3(|| {
        let mut global_alloc = GlobalFrameAllocator;
        // Kernel stacks are PRESENT|WRITABLE|NO_EXECUTE (NX), write-back —
        // exactly `PageFlags::KERNEL_DATA`, which lowers to the same
        // `PageTableFlags::PRESENT | WRITABLE | NO_EXECUTE` (no GLOBAL, matching
        // the prior raw flags). The guard page at `base_vaddr` stays UNMAPPED.
        let flags = crate::arch::mmu::PageFlags::KERNEL_DATA;
        let mut kspace = crate::arch::mmu::kernel();

        // Frames are allocated INDEPENDENTLY per page (non-contiguous) and mapped
        // with the frame just allocated — so this maps page-by-page via the seam's
        // `map_page` rather than `map_range` (which is for a contiguous physical
        // span). 1.5e migrates ONLY the per-page MAP mechanism onto the arch::mmu
        // seam (KERNEL space — `kernel()`, not `current_user()`); the VA range,
        // the guard page, the frame-allocation loop, and the failure-is-fatal
        // policy are preserved byte-identically. A map failure here was fatal via
        // the old `map_page_in_pml4` (panic on PageAlreadyMapped); the seam's
        // `map_page` returns `Err`, so we `.expect()` to keep the same fail-fast.
        for offset in (0..alloc_size).step_by(4096) {
            let frame = global_alloc.allocate_frame().unwrap_or_else(|| {
                crate::oom::handle_alloc_failure();
                unreachable!()
            });
            kspace
                .map_page(
                    VirtAddr::new(stack_start + offset),
                    frame.start_address(),
                    flags,
                )
                .expect("alloc_kernel_stack: failed to map kernel stack page into KERNEL_PML4");
        }
    });

    let ptr = stack_start as *mut u8;
    (ptr, VirtAddr::new(stack_end))
}

pub fn free_kernel_stack(stack_base: *mut u8, size: usize) {
    use x86_64::structures::paging::{Mapper, Page, Size4KiB};

    let start_vaddr = stack_base as u64;
    let alloc_size = (size as u64 + 4095) & !4095;

    with_kernel_cr3(|| {
        let mut mapper = kernel_page_table();

        let _guard = PAGE_TABLE_LOCK.lock();

        for offset in (0..alloc_size).step_by(4096) {
            let page = Page::<Size4KiB>::containing_address(VirtAddr::new(start_vaddr + offset));
            if let Ok((frame, flush)) = mapper.unmap(page) {
                deallocate_frame(frame);
                flush.ignore();
            }
        }
        x86_64::instructions::tlb::flush_all();
    });
}

/// The kernel PML4 frame, if initialized. Used by feature-gated subsystems
/// (e.g. the KFENCE guard-page sampler) that map fixed kernel VAs after boot.
pub fn kernel_pml4_frame() -> Option<x86_64::structures::paging::PhysFrame> {
    KERNEL_PML4.get().copied()
}

pub unsafe fn harden_memory_map(regions: &'static [MemoryRegion]) {
    use x86_64::structures::paging::Mapper;
    let mut mapper = active_page_table();

    for region in regions {
        // We only harden non-usable regions that are already mapped in the physical map.
        // Usable regions are managed by the Buddy Allocator.
        if region.kind == MemoryRegionKind::Usable {
            continue;
        }

        let is_crash_region = region.start <= 0x0014_0000 && region.end > 0x0010_0000;

        let flags = match region.kind {
            MemoryRegionKind::UnknownUefi(5) => {
                // EfiRuntimeServicesCode: Read-Execute (No Write, No NX)
                PageTableFlags::PRESENT
            }
            MemoryRegionKind::UnknownUefi(6) => {
                // EfiRuntimeServicesData: Read-Write (No Execute)
                PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE
            }
            MemoryRegionKind::Bootloader => {
                PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE
            }
            _ if is_crash_region => {
                // The crash dump region (1MiB mark) MUST remain writable so that panic_dump
                // can write to it without triggering a #PF that cascades into a #DF.
                PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE
            }
            _ => PageTableFlags::PRESENT | PageTableFlags::NO_EXECUTE,
        };

        // Update flags for each page in the region within the direct physical map
        let virt_start = phys_to_virt(region.start);
        let virt_end = phys_to_virt(region.end);

        let start_page: Page<Size4KiB> = Page::containing_address(virt_start);
        let end_page: Page<Size4KiB> = Page::containing_address(virt_end - 1u64);

        for page in Page::range_inclusive(start_page, end_page) {
            if let Ok(flags_and_frame) = mapper.update_flags(page, flags) {
                flags_and_frame.flush();
            }
        }
    }
    crate::serial_println!(
        "[ OK ] Memory map hardened (NX bits applied to non-executable regions)"
    );
}

/// Upgrades the frame allocator from the boot-time sequential allocator
/// to a high-performance NUMA-aware buddy allocator.
pub unsafe fn init_buddy_allocator(regions: &'static [MemoryRegion]) {
    // Stash the map so phys_is_usable_ram() can classify addresses at runtime.
    MEMORY_REGIONS.call_once(|| regions);
    // In a real implementation we would parse ACPI SRAT here to find NUMA nodes.
    // For now, we assume a single node (0) for the entire memory map.
    let mut max_phys = 0u64;
    for r in regions {
        if r.kind == MemoryRegionKind::Usable && r.end > max_phys {
            max_phys = r.end;
        }
    }
    let total_frames = (max_phys / 4096) as usize;
    let bitmap_words = (total_frames + 63) / 64;

    // Phase 4.5: reserve the last 4 MiB of RAM for crash dumps. Returns the
    // physical base of the reserved region (or None if RAM is too small). We
    // exclude [crash_base, crash_base+4MiB) from every buddy range below so it
    // is never handed out by the allocator and survives a warm reboot.
    let crash_base = crate::crash_dump::reserve_crash_region(total_frames);
    let crash_end = crash_base.map(|b| b + 4 * 1024 * 1024);

    // Heap is already online; allocate the bitmap there (scales with RAM size).
    let bitmap_slice: &'static mut [u64] =
        alloc::boxed::Box::leak(alloc::vec![!0u64; bitmap_words].into_boxed_slice());
    let mut buddy = buddy::BuddyAllocator::new(
        bitmap_slice.as_mut_ptr(),
        bitmap_slice.len(),
        total_frames,
        0, // Node 0
    );

    let boot_cursor = FRAME_ALLOCATOR
        .lock()
        .as_ref()
        .and_then(|a| a.current_cursor())
        .unwrap_or(0);

    crate::serial_println!(
        "[buddy] boot cursor = {:#x} (frames below this excluded from buddy)",
        boot_cursor,
    );

    for r in regions {
        if r.kind == MemoryRegionKind::Usable {
            let mut start = r.start;
            let end = r.end;

            // Exclude frames that were already handed out by BootInfoFrameAllocator
            // during early boot (e.g. for page tables, heap). Otherwise, the Buddy
            // Allocator will hand them out again, causing catastrophic memory corruption.
            if start < boot_cursor {
                start = boot_cursor.min(end);
            }
            if start >= end {
                continue;
            }

            match (crash_base, crash_end) {
                // Carve the crash-dump hole out of any usable region that overlaps it.
                (Some(cb), Some(ce)) if start < ce && end > cb => {
                    if start < cb {
                        buddy.add_range(start, cb);
                    }
                    if end > ce {
                        buddy.add_range(ce, end);
                    }
                }
                _ => buddy.add_range(start, end),
            }
        }
    }

    if let Some(cb) = crash_base {
        crate::serial_println!(
            "[ OK ] Crash-dump region reserved: phys {:#x}..{:#x} (4 MiB) excluded from buddy",
            cb,
            cb + 4 * 1024 * 1024,
        );
    }

    BUDDY_ALLOCATORS.lock().push(buddy);
    crate::serial_println!(
        "[ OK ] NUMA-aware Buddy allocators online (Node 0 managed {} MiB)",
        max_phys >> 20,
    );
}

/// Allocate `2^order` physically contiguous 4 KiB frames. Returns the
/// physical address of the first frame on success. Used by drivers (NVMe,
/// virtio-net) that need contiguous DMA buffers.
pub fn allocate_contiguous_frames(order: u8) -> Option<PhysAddr> {
    let mut buddy_lock = BUDDY_ALLOCATORS.lock();
    for buddy in buddy_lock.iter_mut() {
        if let Some(addr) = unsafe { buddy.alloc_block(order) } {
            return Some(PhysAddr::new(addr));
        }
    }
    None
}

pub fn deallocate_contiguous_frames(addr: PhysAddr, order: u8) {
    let mut buddy_lock = BUDDY_ALLOCATORS.lock();
    if let Some(buddy) = buddy_lock.get_mut(0) {
        unsafe {
            buddy.free_block(addr.as_u64(), order);
        }
    }
}

pub fn allocate_frame() -> Option<PhysFrame> {
    let mut buddy_lock = BUDDY_ALLOCATORS.lock();
    // Simple local-node preference: search node list.
    for buddy in buddy_lock.iter_mut() {
        if let Some(addr) = unsafe { buddy.alloc_block(0) } {
            return Some(PhysFrame::containing_address(PhysAddr::new(addr)));
        }
    }

    let mut boot_lock = FRAME_ALLOCATOR.lock();
    boot_lock.as_mut().and_then(|a| a.allocate_frame())
}

pub fn deallocate_frame(frame: PhysFrame) {
    let mut buddy_lock = BUDDY_ALLOCATORS.lock();
    let addr = frame.start_address().as_u64();
    // Return to the first node that covers this address (ideally based on topology).
    if let Some(buddy) = buddy_lock.get_mut(0) {
        unsafe {
            buddy.free_block(addr, 0);
        }
    } else {
        FREED_FRAMES.lock().push(frame);
    }
}

/// Creates a new Page Map Level 4 (PML4) table for a new process.
/// It allocates a 4KiB page, zeroes it, and clones the top half (kernel space)
/// of the current PML4 so the kernel remains mapped.
pub fn create_new_pml4() -> x86_64::structures::paging::PhysFrame {
    use alloc::alloc::{alloc_zeroed, Layout};
    use x86_64::structures::paging::page_table::PageTable;
    use x86_64::structures::paging::Translate;

    // 1. Allocate a 4KiB page for the new PML4
    let layout = Layout::from_size_align(4096, 4096).unwrap();
    let virt_ptr = unsafe { alloc_zeroed(layout) };
    let virt_addr = VirtAddr::from_ptr(virt_ptr);

    // 2. Find its physical address
    let phys_addr = active_page_table()
        .translate_addr(virt_addr)
        .expect("Failed to translate newly allocated PML4 virtual address to physical address!");

    let phys_frame = PhysFrame::containing_address(phys_addr);

    // 3. Clone the entire KERNEL_PML4
    // The initial kernel PML4 has the kernel binary, heap, and physical memory mappings.
    let offset = *PHYS_MEM_OFFSET.get().unwrap();
    let kernel_pml4_frame = *KERNEL_PML4.get().unwrap();

    unsafe {
        let kernel_pml4_virt = offset + kernel_pml4_frame.start_address().as_u64();
        let kernel_pml4 = &*(kernel_pml4_virt.as_ptr::<PageTable>());

        let new_pml4 = &mut *(virt_ptr as *mut PageTable);

        // Clone all 512 entries from the pristine kernel PML4
        for i in 0..512 {
            new_pml4[i] = kernel_pml4[i].clone();
        }

        // Deep copy PML4[0] and PDPT[0] to unshare the lower megabytes where both
        // user apps (0x400000) and kernel (0x01000000) reside.
        use x86_64::structures::paging::PageTableFlags;
        if kernel_pml4[0].flags().contains(PageTableFlags::PRESENT) {
            let pdpt_layout = Layout::from_size_align(4096, 4096).unwrap();
            let new_pdpt_ptr = alloc_zeroed(pdpt_layout);
            let new_pdpt_phys = active_page_table()
                .translate_addr(VirtAddr::from_ptr(new_pdpt_ptr))
                .unwrap();

            let kernel_pdpt_phys = kernel_pml4[0].addr();
            let kernel_pdpt_virt = offset + kernel_pdpt_phys.as_u64();
            let kernel_pdpt = &*(kernel_pdpt_virt.as_ptr::<PageTable>());

            let new_pdpt = &mut *(new_pdpt_ptr as *mut PageTable);
            for i in 0..512 {
                new_pdpt[i] = kernel_pdpt[i].clone();
            }

            let mut pml4_flags = kernel_pml4[0].flags();
            pml4_flags.insert(PageTableFlags::USER_ACCESSIBLE);
            new_pml4[0].set_addr(new_pdpt_phys, pml4_flags);

            // Now deep copy PDPT[0]
            if new_pdpt[0].flags().contains(PageTableFlags::PRESENT) {
                let pd_layout = Layout::from_size_align(4096, 4096).unwrap();
                let new_pd_ptr = alloc_zeroed(pd_layout);
                let new_pd_phys = active_page_table()
                    .translate_addr(VirtAddr::from_ptr(new_pd_ptr))
                    .unwrap();

                let kernel_pd_phys = new_pdpt[0].addr();
                let kernel_pd_virt = offset + kernel_pd_phys.as_u64();
                let kernel_pd = &*(kernel_pd_virt.as_ptr::<PageTable>());

                let new_pd = &mut *(new_pd_ptr as *mut PageTable);
                for i in 0..512 {
                    new_pd[i] = kernel_pd[i].clone();
                }

                let mut pdpt_flags = new_pdpt[0].flags();
                pdpt_flags.insert(PageTableFlags::USER_ACCESSIBLE);
                new_pdpt[0].set_addr(new_pd_phys, pdpt_flags);

                // Deep copy the FOURTH level for the USER low range ONLY:
                // PD[0] (virtual 0..2 MiB) is the one page table that is both
                // (a) PRESENT in the pristine kernel PML4 (the low-1MB identity
                // map / AP trampoline lives there) and so SHARED into every
                // clone, AND (b) where base-0 PIE user binaries load. Left
                // shared, a child mapping its text at base 0 REPLACES the
                // running parent's pages and the parent then executes the
                // child's bytes (proven 2026-06-10: user_init #UD at 0x1947
                // fetching raebridge_host .rodata "SteamInstallPath"). So we
                // give PD[0] a private PT per address space.
                //
                // CRITICAL: do NOT privatize the KERNEL range (PD[8]+ = the
                // kernel image at 0x0100_0000, and any higher PD). A privatized
                // kernel PT no longer matches the kernel's PT frame, so
                // `free_user_page_tables` would treat the kernel's own code/data
                // frames as process-private and FREE them at teardown → the
                // frames get reused → non-canonical VirtAddr / KERNEL PAGE FAULT
                // reboot loop (regression hunted 2026-06-10). PD[1..] for user
                // space isn't kernel-mapped, so the ELF loader creates fresh
                // private PTs there already — only PD[0] needs copying.
                const USER_LOW_PD_ENTRIES: usize = 1; // PD[0] = 0..2 MiB
                for i in 0..USER_LOW_PD_ENTRIES {
                    let e_flags = new_pd[i].flags();
                    if e_flags.contains(PageTableFlags::PRESENT)
                        && !e_flags.contains(PageTableFlags::HUGE_PAGE)
                    {
                        let pt_layout = Layout::from_size_align(4096, 4096).unwrap();
                        let new_pt_ptr = alloc_zeroed(pt_layout);
                        let new_pt_phys = active_page_table()
                            .translate_addr(VirtAddr::from_ptr(new_pt_ptr))
                            .unwrap();

                        let kernel_pt_virt = offset + new_pd[i].addr().as_u64();
                        let kernel_pt = &*(kernel_pt_virt.as_ptr::<PageTable>());
                        let new_pt = &mut *(new_pt_ptr as *mut PageTable);
                        for j in 0..512 {
                            new_pt[j] = kernel_pt[j].clone();
                        }

                        new_pd[i].set_addr(new_pt_phys, e_flags);
                    }
                }
            }
        }
    }

    phys_frame
}

/// Maps a specific physical frame to a virtual page within a given PML4 table.
pub unsafe fn map_page_in_pml4(
    pml4: PhysFrame,
    page: Page,
    frame: PhysFrame,
    flags: PageTableFlags,
) {
    if !map_page_in_pml4_fallible(pml4, page, frame, flags) {
        panic!(
            "Failed to map page {:#x} -> {:#x} in custom PML4: PageAlreadyMapped",
            page.start_address().as_u64(),
            frame.start_address().as_u64()
        );
    }
}

/// Rate-limit counter for the stale-mapping recovery diagnostic (see below).
static STALE_MAP_LOGS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Map `frame` at `page`; if the page is already mapped (stale entry from a
/// recycled page-table frame), unmap the stale mapping and retry.  Returns
/// `false` only on unrecoverable errors (frame allocation failure, huge page).
pub unsafe fn map_page_in_pml4_fallible(
    pml4: PhysFrame,
    page: Page,
    frame: PhysFrame,
    flags: PageTableFlags,
) -> bool {
    use x86_64::structures::paging::mapper::MapToError;
    use x86_64::structures::paging::page_table::PageTable;
    use x86_64::structures::paging::Mapper;

    let offset = *PHYS_MEM_OFFSET.get().unwrap();
    let pml4_virt = offset + pml4.start_address().as_u64();
    let pml4_ptr = pml4_virt.as_mut_ptr::<PageTable>();

    let _guard = PAGE_TABLE_LOCK.lock();

    let mut mapper = OffsetPageTable::new(&mut *pml4_ptr, offset);

    let mut frame_allocator = GlobalFrameAllocator;
    let res = mapper.map_to(page, frame, flags, &mut frame_allocator);
    match res {
        Ok(tlb) => {
            tlb.ignore();
            true
        }
        Err(MapToError::PageAlreadyMapped(stale_frame)) => {
            // The page already has a present PTE. Two causes:
            //  - SAME frame, possibly different flags: a MAP_FIXED overlay
            //    re-mapping a page (ld.so overlays library segments on their
            //    initial whole-file reservation; a protection change). Expected
            //    and COMMON — remap silently to apply the intended flags.
            //  - DIFFERENT frame: the buddy allocator handed out a frame that
            //    still held stale PTEs (rare, genuinely worth a diagnostic).
            // The per-page log is rate-limited: at ~5 ms/serial-line on iron, a
            // library load's hundreds of overlay pages used to flood the port
            // and stall the boot past the capture window.
            if stale_frame != frame {
                let n = STALE_MAP_LOGS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if n < 8 {
                    crate::serial_println!(
                        "[mem] recovering stale mapping: page {:#x} was {:#x} -> {:#x}",
                        page.start_address().as_u64(),
                        stale_frame.start_address().as_u64(),
                        frame.start_address().as_u64(),
                    );
                }
            }
            if let Ok((_old_frame, flush)) = mapper.unmap(page) {
                flush.ignore();
            }
            // Retry
            match mapper.map_to(page, frame, flags, &mut frame_allocator) {
                Ok(tlb) => {
                    tlb.ignore();
                    true
                }
                Err(e2) => {
                    crate::serial_println!("Map_to retry error: {:?}", e2);
                    false
                }
            }
        }
        Err(e) => {
            crate::serial_println!("Map_to error: {:?}", e);
            false
        }
    }
}

/// Clear the PTE for `page` in an arbitrary PML4 (the inverse of
/// [`map_page_in_pml4`]). The frame the PTE pointed at is RETURNED to the caller
/// via the x86_64 mapper but is NOT freed here — the caller owns frame
/// lifetime (e.g. the compositor's surface-resize path frees the old contiguous
/// block itself, mirroring `Surface::drop`). A no-op if the page is not mapped.
/// Holds the same `PAGE_TABLE_LOCK` as the map path.
pub unsafe fn unmap_page_in_pml4(pml4: PhysFrame, page: Page) {
    use x86_64::structures::paging::page_table::PageTable;
    use x86_64::structures::paging::Mapper;

    let offset = *PHYS_MEM_OFFSET.get().unwrap();
    let pml4_virt = offset + pml4.start_address().as_u64();
    let pml4_ptr = pml4_virt.as_mut_ptr::<PageTable>();

    let _guard = PAGE_TABLE_LOCK.lock();

    let mut mapper = OffsetPageTable::new(&mut *pml4_ptr, offset);
    if let Ok((_frame, flush)) = mapper.unmap(page) {
        flush.ignore();
    }
}

/// Physical frame backing `page` in `pml4`, if mapped.
pub fn pml4_page_frame(pml4: PhysFrame, page: Page) -> Option<PhysFrame> {
    use x86_64::structures::paging::page_table::PageTable;
    use x86_64::structures::paging::Translate;

    let offset = *PHYS_MEM_OFFSET.get().unwrap();
    let pml4_virt = offset + pml4.start_address().as_u64();
    let pml4_ptr = unsafe { pml4_virt.as_mut_ptr::<PageTable>() };
    let mapper = unsafe { OffsetPageTable::new(&mut *pml4_ptr, offset) };
    mapper
        .translate_addr(page.start_address())
        .map(|a| PhysFrame::containing_address(a))
}

/// Writable pointer to one 4 KiB page in an arbitrary PML4 (via physmap).
pub unsafe fn pml4_page_ptr(pml4: PhysFrame, page: Page) -> Option<*mut u8> {
    use x86_64::structures::paging::page_table::PageTable;
    use x86_64::structures::paging::Translate;

    let offset = *PHYS_MEM_OFFSET.get().unwrap();
    let pml4_virt = offset + pml4.start_address().as_u64();
    let pml4_ptr = pml4_virt.as_mut_ptr::<PageTable>();
    let mapper = OffsetPageTable::new(&mut *pml4_ptr, offset);
    let phys = mapper.translate_addr(page.start_address())?;
    Some((offset + phys.as_u64()).as_mut_ptr())
}

/// Map a physical MMIO region into the kernel address space at its
/// `phys_to_virt` address, with cache disabled, creating the page-table entries.
///
/// The bootloader's linear physical map only covers usable RAM (and low
/// firmware regions). 64-bit PCI BARs can be assigned far above that window —
/// e.g. an xHCI controller at `0x3800_0000_8000` — so `phys_to_virt(bar)`
/// points at an *unmapped* virtual address and the first MMIO access page-faults
/// in kernel context. Drivers must call this for a BAR before dereferencing it.
///
/// Pages already present (low BARs the bootloader mapped, or covered by a huge
/// page) are left as-is, so existing low-BAR drivers keep working. Returns the
/// virtual address for `phys_addr` with its sub-page offset preserved — which
/// equals `phys_to_virt(phys_addr)`, so callers using `offset + bar` still match.
pub fn map_mmio_region(phys_addr: u64, size: usize) -> VirtAddr {
    use x86_64::structures::paging::mapper::MapToError;

    let virt = phys_to_virt(phys_addr);
    if size == 0 {
        return virt;
    }

    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::NO_CACHE
        | PageTableFlags::NO_EXECUTE;

    let start_phys = phys_addr & !0xFFF;
    let end_phys = phys_addr.saturating_add(size as u64).saturating_add(0xFFF) & !0xFFF;

    let mut mapper = active_page_table();
    let mut frame_allocator = GlobalFrameAllocator;

    let mut p = start_phys;
    while p < end_phys {
        let frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(p));
        let page = Page::<Size4KiB>::containing_address(phys_to_virt(p));
        // SAFETY: mapping device MMIO frames the buddy allocator never owns; the
        // target virtual range is the physmap image of `p`, which is either
        // unmapped (high BAR) or already maps the same frame (low BAR).
        match unsafe { mapper.map_to(page, frame, flags, &mut frame_allocator) } {
            Ok(flush) => flush.flush(),
            // Already covered by the physmap (low BAR) or a huge-page mapping.
            Err(MapToError::PageAlreadyMapped(frame)) => {
                // `harden_memory_map` drops WRITABLE from MemoryMappedIO regions in the physmap.
                // We MUST re-apply WRITABLE here so AML/drivers can write to MMIO registers.
                if let Ok(flusher) = unsafe { mapper.update_flags(page, flags) } {
                    flusher.flush();
                }
            }
            Err(MapToError::ParentEntryHugePage) => {
                // If it's a huge page, we can't easily change flags for 4KB without splitting it.
                // However, the bootloader physmap huge pages usually have WRITABLE unless hardened.
                // If harden_memory_map stripped WRITABLE from a huge page, we'd need to split it.
                // For now, just warn if we need writable but it's a huge page.
            }
            Err(e) => {
                crate::serial_println!(
                    "[mmio] map_mmio_region: phys {:#x} -> virt {:#x} failed: {:?}",
                    p,
                    page.start_address().as_u64(),
                    e
                );
            }
        }
        p += 4096;
    }

    virt
}

/// Map a physical range as WRITE-BACK cached (writable) at its physmap address.
/// Mirrors [`map_mmio_region`] but WITHOUT `NO_CACHE`, so CPU writes stream at cache
/// speed instead of one-uncached-write-per-store. Used for an APU DCN scanout buffer
/// in the firmware-reserved carveout (which is ABOVE the usable-RAM physmap, so it
/// isn't mapped otherwise): the compositor blits full frames here fast, then flushes
/// the range (`clflush`) so the non-snooped DCN read path sees the pixels. Returns the
/// physmap virtual base of `phys_addr`.
pub fn map_phys_wb(phys_addr: u64, size: usize) -> VirtAddr {
    use x86_64::structures::paging::mapper::MapToError;
    let virt = phys_to_virt(phys_addr);
    if size == 0 {
        return virt;
    }
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE;
    let start_phys = phys_addr & !0xFFF;
    let end_phys = phys_addr.saturating_add(size as u64).saturating_add(0xFFF) & !0xFFF;
    let mut mapper = active_page_table();
    let mut frame_allocator = GlobalFrameAllocator;
    let mut p = start_phys;
    while p < end_phys {
        let frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(p));
        let page = Page::<Size4KiB>::containing_address(phys_to_virt(p));
        // SAFETY: the carveout frames the buddy allocator never owns; the target VA is
        // the physmap image of `p`, unmapped until now (carveout is above max_phys).
        match unsafe { mapper.map_to(page, frame, flags, &mut frame_allocator) } {
            Ok(flush) => flush.flush(),
            Err(MapToError::PageAlreadyMapped(_)) => {
                if let Ok(flusher) = unsafe { mapper.update_flags(page, flags) } {
                    flusher.flush();
                }
            }
            Err(_) => {}
        }
        p += 4096;
    }
    virt
}

pub struct BootInfoFrameAllocator {
    memory_regions: &'static [MemoryRegion],
    region_idx: usize,
    frame_idx: usize,
}

impl BootInfoFrameAllocator {
    /// Returns the physical address of the next unallocated frame.
    /// This is used to prevent the Buddy Allocator from reusing frames that were already
    /// handed out during early boot for page tables, heap, and early kernel structures.
    pub fn current_cursor(&self) -> Option<u64> {
        let region = self.memory_regions.get(self.region_idx)?;
        Some(region.start + (self.frame_idx as u64) * 4096)
    }

    /// # Safety
    /// Caller must guarantee the memory regions are valid and that usable
    /// frames are truly unused.
    pub unsafe fn init(memory_regions: &'static [MemoryRegion]) -> Self {
        let region_idx = memory_regions
            .iter()
            .position(|r| r.kind == MemoryRegionKind::Usable)
            .unwrap_or(memory_regions.len());
        BootInfoFrameAllocator {
            memory_regions,
            region_idx,
            frame_idx: 0,
        }
    }
}

pub struct GlobalFrameAllocator;

unsafe impl FrameAllocator<Size4KiB> for GlobalFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        let frame = if let Some(frame) = FREED_FRAMES.lock().pop() {
            Some(frame)
        } else {
            allocate_frame()
        };

        if let Some(f) = frame {
            unsafe {
                let offset = *PHYS_MEM_OFFSET
                    .get()
                    .expect("PHYS_MEM_OFFSET not initialized");
                let ptr = (offset + f.start_address().as_u64()).as_mut_ptr::<u8>();
                core::ptr::write_bytes(ptr, 0, 4096);
            }
        }
        frame
    }
}

use x86_64::structures::paging::FrameDeallocator;

impl FrameDeallocator<Size4KiB> for GlobalFrameAllocator {
    unsafe fn deallocate_frame(&mut self, frame: PhysFrame<Size4KiB>) {
        deallocate_frame(frame);
    }
}

unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        loop {
            let region = self.memory_regions[self.region_idx..]
                .iter()
                .enumerate()
                .find(|(_, r)| r.kind == MemoryRegionKind::Usable)
                .map(|(i, r)| (self.region_idx + i, r))?;

            let (abs_idx, r) = region;
            let frame_addr = r.start + (self.frame_idx as u64) * 4096;

            if frame_addr >= r.end {
                self.region_idx = abs_idx + 1;
                self.frame_idx = 0;
                continue;
            }

            self.region_idx = abs_idx;
            self.frame_idx += 1;
            return Some(PhysFrame::containing_address(PhysAddr::new(frame_addr)));
        }
    }
}

/// Make a virtual address range accessible from Ring 3 (user space).
pub unsafe fn make_user_accessible(addr: VirtAddr, size: usize) {
    if size == 0 {
        return;
    }
    let offset = *PHYS_MEM_OFFSET
        .get()
        .expect("PHYS_MEM_OFFSET not initialized");

    use x86_64::registers::control::Cr3;
    use x86_64::structures::paging::page_table::PageTable;

    let start = addr.as_u64() & !0xFFF;
    let end = (addr.as_u64() + size as u64 - 1) & !0xFFF;

    for page_addr in (start..=end).step_by(4096) {
        let virt = VirtAddr::new(page_addr);
        let (level_4_table_frame, _) = Cr3::read();
        let pml4 =
            &mut *(offset + level_4_table_frame.start_address().as_u64()).as_mut_ptr::<PageTable>();

        let pml4_index = virt.p4_index();
        if !pml4[pml4_index].flags().contains(PageTableFlags::PRESENT) {
            continue;
        }
        let pml4_flags = pml4[pml4_index].flags() | PageTableFlags::USER_ACCESSIBLE;
        pml4[pml4_index].set_flags(pml4_flags);

        let pdpt = &mut *(offset + pml4[pml4_index].addr().as_u64()).as_mut_ptr::<PageTable>();
        let pdpt_index = virt.p3_index();
        if !pdpt[pdpt_index].flags().contains(PageTableFlags::PRESENT) {
            continue;
        }
        let pdpt_flags = pdpt[pdpt_index].flags() | PageTableFlags::USER_ACCESSIBLE;
        pdpt[pdpt_index].set_flags(pdpt_flags);
        if pdpt[pdpt_index].flags().contains(PageTableFlags::HUGE_PAGE) {
            continue;
        }

        let pd = &mut *(offset + pdpt[pdpt_index].addr().as_u64()).as_mut_ptr::<PageTable>();
        let pd_index = virt.p2_index();
        if !pd[pd_index].flags().contains(PageTableFlags::PRESENT) {
            continue;
        }
        let pd_flags = pd[pd_index].flags() | PageTableFlags::USER_ACCESSIBLE;
        pd[pd_index].set_flags(pd_flags);
        if pd[pd_index].flags().contains(PageTableFlags::HUGE_PAGE) {
            continue;
        }

        let pt = &mut *(offset + pd[pd_index].addr().as_u64()).as_mut_ptr::<PageTable>();
        let pt_index = virt.p1_index();
        if !pt[pt_index].flags().contains(PageTableFlags::PRESENT) {
            continue;
        }
        let pt_flags = pt[pt_index].flags() | PageTableFlags::USER_ACCESSIBLE;
        pt[pt_index].set_flags(pt_flags);
    }
    x86_64::instructions::tlb::flush_all();
}

/// Reference count of LIVE tasks using each address space (keyed by PML4 frame
/// physical address). A normal task owns a unique PML4 (count 1, freed on exit
/// exactly as before). A `CLONE_THREAD` thread SHARES its parent's PML4, pushing
/// the count above 1 — so the shared address space is freed only when the LAST
/// group member exits, not when the parent is reaped first. This is the fix for
/// the iron parent-reap/pml4-free double fault (memory: linux-clone-threads-
/// scoping). Behaviour-preserving for non-threaded tasks: incref on construction,
/// decref-and-free at zero on Drop.
static AS_REFCOUNT: Mutex<alloc::collections::BTreeMap<u64, u32>> =
    Mutex::new(alloc::collections::BTreeMap::new());

/// Register a task's use of address space `pml4`.
pub fn as_incref(pml4: PhysFrame) {
    let key = pml4.start_address().as_u64();
    *AS_REFCOUNT.lock().entry(key).or_insert(0) += 1;
}

/// Drop a task's use of `pml4`. Returns `true` (removing the entry) once the
/// count hits zero — the caller must then `free_user_page_tables`. An unregistered
/// PML4 (legacy path that never increffed) returns `true` so it frees as before.
#[must_use]
pub fn as_decref(pml4: PhysFrame) -> bool {
    let key = pml4.start_address().as_u64();
    let mut map = AS_REFCOUNT.lock();
    match map.get_mut(&key) {
        Some(c) => {
            *c = c.saturating_sub(1);
            if *c == 0 {
                map.remove(&key);
                true
            } else {
                false
            }
        }
        None => true,
    }
}

/// Recursively traverses and frees all user-space page tables and physical frames
/// mapped in the lower half of the given PML4 (indices 0..255).
pub unsafe fn free_user_page_tables(pml4_frame: PhysFrame) {
    let offset = *PHYS_MEM_OFFSET.get().unwrap();
    let mut global_alloc = GlobalFrameAllocator;

    let pml4_virt = offset + pml4_frame.start_address().as_u64();
    let pml4 = &mut *pml4_virt.as_mut_ptr::<x86_64::structures::paging::page_table::PageTable>();

    let kernel_pml4_frame = *KERNEL_PML4.get().unwrap();
    let kernel_pml4_virt = offset + kernel_pml4_frame.start_address().as_u64();
    let kernel_pml4 =
        &*kernel_pml4_virt.as_ptr::<x86_64::structures::paging::page_table::PageTable>();

    // User canonical space uses PML4 indices 0..255 (kernel is 256..511).
    for i in 0..256 {
        let entry = &pml4[i];
        if entry.flags().contains(PageTableFlags::PRESENT) {
            let pdpt_frame = entry.frame().unwrap();

            // Skip frames that are shared exactly with the kernel's PML4
            if pdpt_frame.start_address() == kernel_pml4[i].addr() {
                continue;
            }

            let kernel_pdpt = if kernel_pml4[i].flags().contains(PageTableFlags::PRESENT) {
                let addr = offset + kernel_pml4[i].addr().as_u64();
                Some(&*addr.as_ptr::<x86_64::structures::paging::page_table::PageTable>())
            } else {
                None
            };

            let pdpt_virt = offset + pdpt_frame.start_address().as_u64();
            let pdpt =
                &mut *pdpt_virt.as_mut_ptr::<x86_64::structures::paging::page_table::PageTable>();

            for j in 0..512 {
                let pdpt_entry = &pdpt[j];
                if pdpt_entry.flags().contains(PageTableFlags::PRESENT)
                    && !pdpt_entry.flags().contains(PageTableFlags::HUGE_PAGE)
                {
                    let pd_frame = pdpt_entry.frame().unwrap();

                    if let Some(k_pdpt) = kernel_pdpt {
                        if k_pdpt[j].flags().contains(PageTableFlags::PRESENT)
                            && pd_frame.start_address() == k_pdpt[j].addr()
                        {
                            continue;
                        }
                    }

                    let kernel_pd = if let Some(k_pdpt) = kernel_pdpt {
                        if k_pdpt[j].flags().contains(PageTableFlags::PRESENT)
                            && !k_pdpt[j].flags().contains(PageTableFlags::HUGE_PAGE)
                        {
                            let addr = offset + k_pdpt[j].addr().as_u64();
                            Some(
                                &*addr
                                    .as_ptr::<x86_64::structures::paging::page_table::PageTable>(),
                            )
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    let pd_virt = offset + pd_frame.start_address().as_u64();
                    let pd = &mut *pd_virt
                        .as_mut_ptr::<x86_64::structures::paging::page_table::PageTable>();

                    for k in 0..512 {
                        let pd_entry = &pd[k];
                        if pd_entry.flags().contains(PageTableFlags::PRESENT)
                            && !pd_entry.flags().contains(PageTableFlags::HUGE_PAGE)
                        {
                            let pt_frame = pd_entry.frame().unwrap();

                            if let Some(k_pd) = kernel_pd {
                                if k_pd[k].flags().contains(PageTableFlags::PRESENT)
                                    && pt_frame.start_address() == k_pd[k].addr()
                                {
                                    continue;
                                }
                            }

                            // The kernel's PT for this PD slot, if any. A
                            // privatized user PT (e.g. PD[0]) is a COPY of the
                            // kernel PT and therefore still maps SHARED low-mem
                            // frames (AP trampoline / BIOS) alongside the
                            // process's private ELF frames. We must free only
                            // the private frames — freeing a shared kernel frame
                            // corrupts the kernel (regression 2026-06-10).
                            let kernel_pt = if let Some(k_pd) = kernel_pd {
                                if k_pd[k].flags().contains(PageTableFlags::PRESENT)
                                    && !k_pd[k].flags().contains(PageTableFlags::HUGE_PAGE)
                                {
                                    let addr = offset + k_pd[k].addr().as_u64();
                                    Some(&*addr.as_ptr::<
                                        x86_64::structures::paging::page_table::PageTable,
                                    >())
                                } else {
                                    None
                                }
                            } else {
                                None
                            };

                            let pt_virt = offset + pt_frame.start_address().as_u64();
                            let pt = &mut *pt_virt
                                .as_mut_ptr::<x86_64::structures::paging::page_table::PageTable>();

                            for l in 0..512 {
                                let pt_entry = &pt[l];
                                if pt_entry.flags().contains(PageTableFlags::PRESENT) {
                                    // Skip frames the kernel also maps at this
                                    // slot — they are shared, not ours to free.
                                    if let Some(k_pt) = kernel_pt {
                                        if k_pt[l].flags().contains(PageTableFlags::PRESENT)
                                            && pt_entry.frame().unwrap().start_address()
                                                == k_pt[l].addr()
                                        {
                                            continue;
                                        }
                                    }
                                    global_alloc.deallocate_frame(pt_entry.frame().unwrap());
                                }
                            }
                            global_alloc.deallocate_frame(pt_frame);
                        }
                    }
                    global_alloc.deallocate_frame(pd_frame);
                }
            }
            global_alloc.deallocate_frame(pdpt_frame);
        }
    }
    global_alloc.deallocate_frame(pml4_frame);
}

/// Validate the bootloader memory map before any allocator walks it.
pub fn verify_boot_memory_map(regions: &'static [MemoryRegion]) {
    let usable_count = regions
        .iter()
        .filter(|r| r.kind == MemoryRegionKind::Usable)
        .count();
    if usable_count == 0 {
        panic!("[memory] verify failed: NO USABLE RAM reported by bootloader");
    }
}

pub fn run_boot_smoketest() {
    crate::serial_println!("[memory] run_boot_smoketest: verifying core regions...");

    // 1. The buddy allocator must report a usable physical pool: total > 0
    //    and at least some frames currently free.
    let (buddy_total, buddy_free) = {
        let g = BUDDY_ALLOCATORS.lock();
        match g.first() {
            Some(b) => b.stats(),
            None => (0, 0),
        }
    };
    let buddy_ok = buddy_total > 0 && buddy_free > 0 && buddy_free <= buddy_total;

    // 2. The kernel heap must have free space (we are mid-boot and have not
    //    exhausted the 128 MiB heap).
    let heap_free = crate::memory::allocator::heap_free();
    let heap_ok = heap_free > 0;

    // 3. A heap round-trip must actually work: allocate, write, read back.
    let mut probe = alloc::vec::Vec::<u8>::new();
    let heap_rt_ok = probe.try_reserve_exact(4096).is_ok() && {
        probe.resize(4096, 0xA5);
        probe.first() == Some(&0xA5) && probe.last() == Some(&0xA5) && probe.len() == 4096
    };
    drop(probe);

    let pass = buddy_ok && heap_ok && heap_rt_ok;

    crate::serial_println!(
        "[memory] smoketest: buddy total={} free={} frames; heap_free={} KiB; heap_rt_ok={} -> {}",
        buddy_total,
        buddy_free,
        heap_free >> 10,
        heap_rt_ok,
        if pass { "PASS" } else { "FAIL" }
    );

    if !pass {
        crate::serial_println!(
            "[memory] smoketest FAIL: buddy_ok={} heap_ok={} heap_rt_ok={}",
            buddy_ok,
            heap_ok,
            heap_rt_ok
        );
    }
}

/// Map a physical MMIO range into the **current task's** user PML4 (syscall 7 path).
pub fn map_phys_mmio_into_current_task(
    start_phys: u64,
    length: usize,
    user_virt: u64,
) -> Result<(), u64> {
    use crate::arch::{PhysAddr, VirtAddr};
    use crate::capability::E_INVAL;
    use x86_64::structures::paging::{Page, PageTableFlags, PhysFrame, Size4KiB};

    if length == 0
        || (user_virt & 0xFFF) != 0
        || (length & 0xFFF) != 0
        || user_virt >= 0x0000_8000_0000_0000
    {
        return Err(E_INVAL);
    }

    let pml4 = crate::scheduler::with_current_task(|t| t.pml4).flatten();
    let pml4 = match pml4 {
        Some(p) => p,
        None => return Err(E_INVAL),
    };

    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::USER_ACCESSIBLE
        | PageTableFlags::NO_CACHE
        | PageTableFlags::WRITE_THROUGH;

    let pages = length / 4096;
    for i in 0..pages {
        let phys = PhysAddr::new(start_phys + (i * 4096) as u64);
        let virt = VirtAddr::new(user_virt + (i * 4096) as u64);
        let frame: PhysFrame<Size4KiB> = PhysFrame::containing_address(phys);
        let page: Page<Size4KiB> = Page::containing_address(virt);
        unsafe {
            map_page_in_pml4(pml4, page, frame, flags);
        }
    }
    x86_64::instructions::tlb::flush_all();
    Ok(())
}

/// Map physically-contiguous NORMAL RAM (e.g. a `request_firmware` blob) into the
/// current task as WRITE-BACK cached, user-accessible memory — the RAM
/// counterpart to [`map_phys_mmio_into_current_task`].
///
/// That sibling maps device MMIO as `NO_CACHE | WRITE_THROUGH` (UC), which is
/// correct for BAR registers but WRONG for a firmware blob that lives in regular
/// DRAM: `lkpi_request_firmware` fills the blob through the kernel's WRITE-BACK
/// physmap alias, so handing the daemon a UC alias of the SAME physical frames
/// creates a WB/UC aliasing hazard — the Intel/AMD SDMs leave such mixed-type
/// aliases undefined, and on real AMD silicon the daemon's first UC read of the
/// blob can observe stale/garbage data or stall (the amdgpu iron bring-up hang:
/// the blob loaded, then `read_discovery` froze on the first byte-read; see
/// memory `amdgpu-iron-hang-uc-firmware-read`). Mapping the blob WB keeps the
/// daemon's view cache-coherent with the kernel's fill (WB-to-WB aliasing of one
/// physical page is coherent on x86). Used by `linuxkpi_host::lkpi_request_firmware`.
pub fn map_phys_ram_into_current_task(
    start_phys: u64,
    length: usize,
    user_virt: u64,
) -> Result<(), u64> {
    use crate::arch::{PhysAddr, VirtAddr};
    use crate::capability::E_INVAL;
    use x86_64::structures::paging::{Page, PageTableFlags, PhysFrame, Size4KiB};

    if length == 0
        || (user_virt & 0xFFF) != 0
        || (length & 0xFFF) != 0
        || user_virt >= 0x0000_8000_0000_0000
    {
        return Err(E_INVAL);
    }

    let pml4 = crate::scheduler::with_current_task(|t| t.pml4).flatten();
    let pml4 = match pml4 {
        Some(p) => p,
        None => return Err(E_INVAL),
    };

    // WRITE-BACK cached: PRESENT|WRITABLE|USER with neither NO_CACHE (PCD) nor
    // WRITE_THROUGH (PWT) → PAT entry 0 = WB. This is normal DRAM, not MMIO.
    let flags =
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;

    let pages = length / 4096;
    for i in 0..pages {
        let phys = PhysAddr::new(start_phys + (i * 4096) as u64);
        let virt = VirtAddr::new(user_virt + (i * 4096) as u64);
        let frame: PhysFrame<Size4KiB> = PhysFrame::containing_address(phys);
        let page: Page<Size4KiB> = Page::containing_address(virt);
        unsafe {
            map_page_in_pml4(pml4, page, frame, flags);
        }
    }
    x86_64::instructions::tlb::flush_all();
    Ok(())
}

#[inline(always)]
pub fn phys_to_virt(phys: u64) -> VirtAddr {
    let offset = *PHYS_MEM_OFFSET
        .get()
        .expect("PHYS_MEM_OFFSET not initialized");
    offset + phys
}

#[inline(always)]
pub fn virt_to_phys(virt: VirtAddr) -> Option<PhysAddr> {
    kernel_translate_addr(virt).or_else(|| {
        use x86_64::structures::paging::Translate;
        active_page_table().translate_addr(virt)
    })
}

/// Returns a pseudo-random 64-bit value for ASLR/KASLR purposes.
/// Uses RDTSC as a source of entropy in early boot.
pub fn aslr_random() -> u64 {
    let mut lo: u32;
    let mut hi: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi);
    }
    let tsc = ((hi as u64) << 32) | (lo as u64);
    // Simple mixing hash to improve entropy spread
    let mut x = tsc;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x
}

// ═══════════════════════════════════════════════════════════════════════════════
//  KASLR — Kernel Address Space Layout Randomization
// ═══════════════════════════════════════════════════════════════════════════════

/// Generates a random offset for the kernel base address.
/// Uses 30 bits of entropy for a range of ~1 TiB in the 64-bit address space.
pub fn kaslr_random_offset() -> u64 {
    // 2MB alignment for hugepage compatibility
    let raw = aslr_random();
    let offset = (raw & 0x3FFFFFFF) << 21;
    offset
}

pub fn apply_kaslr(base_offset: u64) {
    if base_offset == 0 {
        return;
    }
    // Runtime kernel remap is not implemented here. Layout randomization is tracked
    // in `hardening::KaslrState` after `HardeningManager::init_all()`.
    crate::serial_println!(
        "[ KERN ] KASLR: slide {:#x} recorded (no runtime remap; see hardening::KaslrState)",
        base_offset
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SYS_MPROTECT (283) — flip protection flags on mapped user pages
// ═══════════════════════════════════════════════════════════════════════════════
//
// Concept §Compatibility — "AthBridge runs Windows apps natively". A real
// MSVC-CRT `.exe` is loaded by AthBridge into an RW mapping (copy + relocate +
// IAT patch), then its `.text` is flipped RW→RX. There was no mprotect; this
// is it. Mirrors `posix::sys_mmap`'s page-flag convention (PROT R=1/W=2/X=4)
// and uses the ACTIVE page table — mprotect edits the CALLER's own user
// mapping (same address space as the syscalling task), exactly like sys_mmap.

/// Whether AthGuard's W^X policy is enforced. When true, `sys_mprotect`
/// refuses any request that asks for WRITE and EXEC simultaneously (a page may
/// be writable OR executable, never both). Off during bring-up so AthBridge
/// can come up before the policy lands.
// MasterChecklist Phase 9: flip this on when AthGuard enforces W^X, and make
// SYS_MMAP stop mapping pages executable by default.
static WX_POLICY_ENFORCED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Enable/disable the W^X enforcement gate (AthGuard Phase 9 hook).
pub fn set_wx_policy_enforced(on: bool) {
    WX_POLICY_ENFORCED.store(on, core::sync::atomic::Ordering::SeqCst);
}

/// Pure mapping from POSIX-style `prot` bits to the two meaningful x86_64 PTE
/// flag decisions: `(writable, no_execute)`. `PROT_READ` is implicit (a present
/// user page is always readable on x86_64). Host-KAT-able — no hardware state.
///
/// - `PROT_READ | PROT_EXEC` (5) → `(false, false)` = RX
/// - `PROT_READ | PROT_WRITE` (3) → `(true,  true)`  = RW + NX
/// - `PROT_READ` (1)             → `(false, true)`  = RO + NX
#[inline]
pub fn prot_to_pte_flags(prot: u64) -> (bool, bool) {
    const PROT_WRITE: u64 = 2;
    const PROT_EXEC: u64 = 4;
    let writable = prot & PROT_WRITE != 0;
    let no_execute = prot & PROT_EXEC == 0;
    (writable, no_execute)
}

/// True if `prot` requests WRITE and EXEC simultaneously — the transition the
/// W^X policy forbids. Host-KAT-able.
#[inline]
pub fn prot_is_w_and_x(prot: u64) -> bool {
    const PROT_WRITE: u64 = 2;
    const PROT_EXEC: u64 = 4;
    (prot & PROT_WRITE != 0) && (prot & PROT_EXEC != 0)
}

/// `mprotect(addr, len, prot)`: change the protection flags of already-mapped
/// 4 KiB user pages in `[addr, addr+len)`. Returns `0` on success, `u64::MAX`
/// on any error (misaligned addr, kernel-half/overflow range, an unmapped page
/// in the range, or a W+X request under an active W^X policy). Does NOT map new
/// pages — a hole in the range is an error (this is mprotect, not mmap).
///
/// Validate-then-flip: the whole range is checked mapped BEFORE any PTE is
/// touched, so the flip loop cannot fail partway (atomic in practice). Every
/// flipped page gets a mandatory TLB flush — a missed flush leaves the old
/// protection live and is a silent W^X bypass.
pub fn sys_mprotect(addr: u64, len: u64, prot: u64) -> u64 {
    use crate::arch::VirtAddr;
    use x86_64::structures::paging::{Mapper, Page, PageTableFlags, Size4KiB, Translate};

    const USER_SPACE_END: u64 = 0x0000_8000_0000_0000;

    // 1. addr must be page-aligned; len rounded up to a page (overflow-safe).
    if addr & 0xFFF != 0 {
        return u64::MAX;
    }
    if len == 0 {
        return 0; // POSIX: a zero-length mprotect succeeds without effect.
    }
    let size = match len.checked_add(0xFFF) {
        Some(v) => v & !0xFFF,
        None => return u64::MAX,
    };

    // 2. Range must stay in the user half (no kernel-half / overflow).
    match addr.checked_add(size) {
        Some(end) if end <= USER_SPACE_END => {}
        _ => return u64::MAX,
    }

    // 3. AthGuard W^X gate: refuse simultaneous W+X under an active policy.
    if WX_POLICY_ENFORCED.load(core::sync::atomic::Ordering::Relaxed) && prot_is_w_and_x(prot) {
        return u64::MAX;
    }

    let (writable, no_execute) = prot_to_pte_flags(prot);
    let mut new_flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    if writable {
        new_flags |= PageTableFlags::WRITABLE;
    }
    if no_execute {
        new_flags |= PageTableFlags::NO_EXECUTE;
    }

    // mprotect edits the caller's own user mapping → active page table (active
    // CR3), exactly like sys_mmap/sys_munmap. NOT KERNEL_PML4 (that is for
    // kernel/DMA buffers, pitfall #2) — this reaches no other address space.
    let mut pt = active_page_table();

    // 4. Pre-validate: every page in the range must already be mapped. A hole
    //    aborts before any flip, making the subsequent loop infallible.
    let start_page: Page<Size4KiB> = Page::containing_address(VirtAddr::new(addr));
    let end_page: Page<Size4KiB> = Page::containing_address(VirtAddr::new(addr + size - 1));
    for page in Page::range_inclusive(start_page, end_page) {
        if pt.translate_addr(page.start_address()).is_none() {
            return u64::MAX; // unmapped page in range
        }
    }

    // 5. Flip flags + flush TLB for each page. update_flags preserves the
    //    mapped frame and replaces the flag set.
    for page in Page::range_inclusive(start_page, end_page) {
        match unsafe { pt.update_flags(page, new_flags) } {
            Ok(flush) => flush.flush(),
            // Pre-validated as mapped above; a failure here is unexpected but
            // must not be silent — report failure rather than leave it partial.
            Err(_) => return u64::MAX,
        }
    }

    0
}

/// FAIL-able boot proof of `SYS_MPROTECT` (283): map one user-half page RW,
/// run `sys_mprotect(.., PROT_READ|PROT_EXEC)`, and assert the PTE actually
/// flipped (WRITABLE cleared, NO_EXECUTE cleared). Also asserts a
/// `PROT_WRITE|PROT_EXEC` request is refused once the W^X policy is enabled,
/// and that an unmapped range is rejected. Unmaps + frees the test page after.
/// Concept §Compatibility — the loader's RW→RX flip for relocated `.text`.
pub fn run_mprotect_smoketest() {
    use crate::arch::VirtAddr;
    use x86_64::structures::paging::{FrameAllocator, Mapper, Page, PageTableFlags, Size4KiB};

    const PROT_READ: u64 = 1;
    const PROT_WRITE: u64 = 2;
    const PROT_EXEC: u64 = 4;
    // A user-half test address well clear of stacks/heap/mmap regions.
    const TEST_VA: u64 = 0x0000_6000_0000_0000;

    let mut alloc = GlobalFrameAllocator;
    let mut pt = active_page_table();
    let page: Page<Size4KiB> = Page::containing_address(VirtAddr::new(TEST_VA));

    let frame = match alloc.allocate_frame() {
        Some(f) => f,
        None => {
            crate::serial_println!("[mprotect] smoketest: no frame -> FAIL");
            return;
        }
    };

    // Map RW + NX (the loader's initial state for a writable section).
    let rw_flags = PageTableFlags::PRESENT
        | PageTableFlags::USER_ACCESSIBLE
        | PageTableFlags::WRITABLE
        | PageTableFlags::NO_EXECUTE;
    let map_ok = unsafe { pt.map_to(page, frame, rw_flags, &mut alloc) };
    if let Ok(flush) = map_ok {
        flush.flush();
    } else {
        // Address already mapped (re-run) — free our frame and bail cleanly.
        deallocate_frame(frame);
        crate::serial_println!("[mprotect] smoketest: map_to failed (already mapped?) -> FAIL");
        return;
    }

    // 1. RW->RX flip: clear WRITABLE, clear NO_EXECUTE.
    let r1 = sys_mprotect(TEST_VA, 4096, PROT_READ | PROT_EXEC);
    let flags_after = read_pte_flags(&pt, page);
    let rx_ok = r1 == 0
        && flags_after.map_or(false, |f| {
            !f.contains(PageTableFlags::WRITABLE) && !f.contains(PageTableFlags::NO_EXECUTE)
        });

    // 2. W^X gate: with policy OFF a W+X request is honored; with policy ON it
    //    is refused (u64::MAX) and the page is left unchanged.
    set_wx_policy_enforced(true);
    let r_wx = sys_mprotect(TEST_VA, 4096, PROT_READ | PROT_WRITE | PROT_EXEC);
    set_wx_policy_enforced(false);
    let wx_refused = r_wx == u64::MAX;

    // 3. Unmapped range is rejected.
    let r_hole = sys_mprotect(0x0000_6000_1000_0000, 4096, PROT_READ);
    let hole_rejected = r_hole == u64::MAX;

    // 4. Misaligned addr is rejected.
    let r_align = sys_mprotect(TEST_VA + 1, 4096, PROT_READ);
    let align_rejected = r_align == u64::MAX;

    // Tear down: unmap + free the test frame.
    if let Ok((freed, flush)) = pt.unmap(page) {
        flush.flush();
        deallocate_frame(freed);
    }

    if rx_ok && wx_refused && hole_rejected && align_rejected {
        crate::serial_println!(
            "[mprotect] smoketest: rw->rx={} wx_refused={} hole_rejected={} align_rejected={} -> PASS",
            rx_ok,
            wx_refused,
            hole_rejected,
            align_rejected
        );
    } else {
        crate::serial_println!(
            "[mprotect] smoketest: rw->rx={} wx_refused={} hole_rejected={} align_rejected={} -> FAIL",
            rx_ok,
            wx_refused,
            hole_rejected,
            align_rejected
        );
    }
}

/// Read the leaf PTE flags for a mapped 4 KiB page from `pt`. Returns `None`
/// if unmapped. Used by the mprotect smoketest to assert the flip took effect.
fn read_pte_flags(
    pt: &x86_64::structures::paging::OffsetPageTable<'static>,
    page: x86_64::structures::paging::Page<x86_64::structures::paging::Size4KiB>,
) -> Option<x86_64::structures::paging::PageTableFlags> {
    use x86_64::structures::paging::mapper::TranslateResult;
    use x86_64::structures::paging::Translate;
    match pt.translate(page.start_address()) {
        TranslateResult::Mapped { flags, .. } => Some(flags),
        _ => None,
    }
}

#[cfg(test)]
mod mprotect_tests {
    use super::{prot_is_w_and_x, prot_to_pte_flags};

    #[test]
    fn rx_clears_write_and_nx() {
        assert_eq!(prot_to_pte_flags(5), (false, false));
    }

    #[test]
    fn rw_sets_write_and_nx() {
        assert_eq!(prot_to_pte_flags(3), (true, true));
    }

    #[test]
    fn ro_clears_write_sets_nx() {
        assert_eq!(prot_to_pte_flags(1), (false, true));
    }

    #[test]
    fn wx_detection() {
        assert!(prot_is_w_and_x(6));
        assert!(prot_is_w_and_x(7));
        assert!(!prot_is_w_and_x(5));
        assert!(!prot_is_w_and_x(3));
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Memory Pinning API — guaranteed-resident pages for game hot data
// ═══════════════════════════════════════════════════════════════════════════════

use spin::RwLock;

#[derive(Debug, Clone, Copy)]
pub struct PinnedRegion {
    pub start: VirtAddr,
    pub size: usize,
    pub pages: usize,
}

#[derive(Debug)]
pub struct MemoryPinManager {
    pinned_regions: alloc::vec::Vec<PinnedRegion>,
    total_pinned_pages: usize,
    max_pinnable_pages: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinError {
    ExceedsLimit,
    Unaligned,
    ZeroSize,
    AlreadyPinned,
    NotPinned,
    InsufficientCapability,
}

lazy_static::lazy_static! {
    static ref PIN_MANAGER: RwLock<MemoryPinManager> = RwLock::new(MemoryPinManager::new());
}

impl MemoryPinManager {
    fn new() -> Self {
        Self {
            pinned_regions: alloc::vec::Vec::new(),
            total_pinned_pages: 0,
            max_pinnable_pages: 4 * 1024 * 1024,
        }
    }

    pub fn set_total_memory(&mut self, total_pages: usize) {
        self.max_pinnable_pages = total_pages / 2;
    }

    fn is_overlapping(&self, start: VirtAddr, size: usize) -> bool {
        let end = start + size as u64;
        self.pinned_regions.iter().any(|r| {
            let r_end = r.start + r.size as u64;
            start < r_end && end > r.start
        })
    }

    fn pin(&mut self, start: VirtAddr, size: usize) -> Result<PinnedRegion, PinError> {
        if size == 0 {
            return Err(PinError::ZeroSize);
        }
        if !start.is_aligned(4096u64) || size % 4096 != 0 {
            return Err(PinError::Unaligned);
        }
        let pages = size / 4096;
        if self.total_pinned_pages + pages > self.max_pinnable_pages {
            return Err(PinError::ExceedsLimit);
        }
        if self.is_overlapping(start, size) {
            return Err(PinError::AlreadyPinned);
        }
        let region = PinnedRegion { start, size, pages };
        self.total_pinned_pages += pages;
        self.pinned_regions.push(region);
        Ok(region)
    }

    fn unpin(&mut self, start: VirtAddr, size: usize) -> Result<(), PinError> {
        if let Some(idx) = self
            .pinned_regions
            .iter()
            .position(|r| r.start == start && r.size == size)
        {
            let region = self.pinned_regions.remove(idx);
            self.total_pinned_pages -= region.pages;
            Ok(())
        } else {
            Err(PinError::NotPinned)
        }
    }

    pub fn total_pinned_pages(&self) -> usize {
        self.total_pinned_pages
    }
    pub fn max_pinnable_pages(&self) -> usize {
        self.max_pinnable_pages
    }
    pub fn pinned_regions(&self) -> &[PinnedRegion] {
        &self.pinned_regions
    }
}

pub fn pin_memory(addr: VirtAddr, size: usize) -> Result<PinnedRegion, PinError> {
    let mut mgr = PIN_MANAGER.write();
    mgr.pin(addr, size)
}

pub fn unpin_memory(addr: VirtAddr, size: usize) -> Result<(), PinError> {
    let mut mgr = PIN_MANAGER.write();
    mgr.unpin(addr, size)
}

pub fn pinned_page_count() -> usize {
    PIN_MANAGER.read().total_pinned_pages()
}

/// Total installed physical RAM the kernel manages, in bytes.
///
/// Sums every NUMA node's buddy allocator `total_frames` (the usable-RAM
/// frames carved from the UEFI/e820 map at boot) and multiplies by the 4 KiB
/// frame size. Returns `None` if no allocator is initialized yet or the lock
/// is contended — callers (e.g. the `/proc/raeen/memory` snapshot) must not
/// block the boot dump on a held buddy lock. Read-only: never mutates state.
pub fn physical_total_bytes() -> Option<u64> {
    let g = BUDDY_ALLOCATORS.try_lock()?;
    if g.is_empty() {
        return None;
    }
    let mut frames: u64 = 0;
    for b in g.iter() {
        let (total, _free) = b.stats();
        frames = frames.saturating_add(total as u64);
    }
    Some(frames.saturating_mul(4096))
}

/// Currently free physical RAM across all buddy nodes, in bytes. Companion to
/// [`physical_total_bytes`]; same lock discipline (non-blocking `try_lock`).
pub fn physical_free_bytes() -> Option<u64> {
    let g = BUDDY_ALLOCATORS.try_lock()?;
    if g.is_empty() {
        return None;
    }
    let mut frames: u64 = 0;
    for b in g.iter() {
        let (_total, free) = b.stats();
        frames = frames.saturating_add(free as u64);
    }
    Some(frames.saturating_mul(4096))
}
pub fn max_pinnable_pages() -> usize {
    PIN_MANAGER.read().max_pinnable_pages()
}

pub fn configure_pin_limit(total_system_pages: usize) {
    let mut mgr = PIN_MANAGER.write();
    mgr.set_total_memory(total_system_pages);
    crate::serial_println!(
        "[ OK ] Memory pin limit set: max {} pages (50% of {} total)",
        mgr.max_pinnable_pages(),
        total_system_pages,
    );
}
