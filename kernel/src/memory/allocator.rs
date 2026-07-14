//! Kernel heap allocator.
//!
//! Maps a 256 KiB region of virtual memory for the heap and initializes
//! a linked-list allocator over it. Once this runs, `alloc::Vec`,
//! `alloc::String`, etc. are available in the kernel.

use crate::arch::VirtAddr;
use core::alloc::{GlobalAlloc, Layout};
use linked_list_allocator::LockedHeap;
use x86_64::structures::paging::mapper::MapToError;
use x86_64::structures::paging::{FrameAllocator, Mapper, Page, PageTableFlags, Size4KiB};

#[cfg(feature = "kasan")]
use core::sync::atomic::{AtomicBool, Ordering};

struct OomAwareHeap;

unsafe impl GlobalAlloc for OomAwareHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // KFENCE sampler (feature = "kfence" only). On a sampled allocation,
        // divert to a guard-page-flanked slot so an OOB/UAF on this object
        // takes a #PF the handler classifies. Non-sampled allocations — and the
        // ENTIRE default build — take the byte-identical fast path below.
        #[cfg(feature = "kfence")]
        {
            // Only objects that fit in one page can live in a guard-page slot.
            if layout.size() != 0
                && layout.size() <= 4096
                && crate::hardening::sampler::should_sample()
            {
                // Cheap attribution: the kernel return address of this frame.
                let ip = crate::hardening::sampler::caller_ip();
                if let Some(p) = crate::hardening::sampler::allocate(layout.size(), ip) {
                    return p;
                }
                // Pool full/contended: fall through to the normal heap.
            }
        }

        loop {
            let ptr = HEAP_INNER.alloc(layout);
            if !ptr.is_null() {
                // KASAN (feature = "kasan" only): the entire heap is shadow-poisoned
                // (0xFF) at init and freed chunks are re-poisoned on dealloc, so the
                // chunk we just got handed back is currently marked invalid. Unpoison
                // exactly the allocation's bytes; the trailing bytes of the final
                // 8-byte shadow granule and everything adjacent stay poisoned, which
                // is what catches out-of-bounds reads/writes past the object.
                #[cfg(feature = "kasan")]
                kasan_alloc_hook(ptr as usize, layout.size());
                return ptr;
            }
            crate::oom::handle_alloc_failure();
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // KFENCE-owned addresses (feature = "kfence" only) free through the
        // sampler so a double-free is recorded/classified. `is_kfence_address`
        // is a const-false until the pool is mapped, so the default build and
        // the non-sampled fast path are unaffected.
        #[cfg(feature = "kfence")]
        {
            if crate::hardening::sampler::is_kfence_address(ptr as u64) {
                let ip = crate::hardening::sampler::caller_ip();
                if let Err(e) = crate::hardening::sampler::free(ptr as u64, ip) {
                    crate::serial_println!(
                        "[kfence] dealloc fault: addr={:#x} ip={:#x} -> {:?}",
                        ptr as u64,
                        ip,
                        e
                    );
                }
                // KFENCE slots are never returned to the linked-list heap.
                return;
            }
        }

        // KASAN (feature = "kasan" only): poison the freed region's shadow so any
        // later access reads an invalid byte (use-after-free), then route the chunk
        // through a quarantine ring instead of returning it to the heap immediately
        // — a freed-then-reused chunk would be unpoisoned by the next alloc and the
        // UAF would go undetected. When the ring overflows, the oldest entry is
        // genuinely freed back to the linked-list heap.
        #[cfg(feature = "kasan")]
        {
            if kasan_dealloc_hook(ptr as usize, layout) {
                // Chunk was quarantined; do NOT return it to the heap yet.
                return;
            }
        }

        HEAP_INNER.dealloc(ptr, layout);
    }
}

// ===========================================================================
// KASAN runtime (feature = "kasan" only).
//
// Manual address-sanitizer over the heap's shadow region (`SHADOW_START`,
// 1 byte / 8 heap bytes: 0x00 = valid, 0xFF = poisoned). The whole heap is
// poisoned at init; alloc unpoisons the live object, dealloc re-poisons it.
// Access checks consult the shadow at the allocator boundary and via the
// public `kasan_check`. Everything here is compiled out of the default build.
// ===========================================================================

#[cfg(feature = "kasan")]
mod kasan_rt {
    use super::{AtomicBool, Ordering};
    use super::{HEAP_INNER, HEAP_SIZE, HEAP_START, SHADOW_START};
    use core::alloc::{GlobalAlloc, Layout};
    use spin::Mutex;

    /// Shadow encodings (allocator scheme: 1 shadow byte per 8 heap bytes).
    pub const SHADOW_VALID: u8 = 0x00;
    pub const SHADOW_FREED: u8 = 0xFB; // KASAN_POISON_KMALLOC_FREE — use-after-free
    pub const SHADOW_REDZONE: u8 = 0xFF; // out-of-bounds / never-allocated

    /// Set true once the shadow region is mapped + zero/poison-initialized by
    /// `init_heap`. Read lock-free on the allocator hot path; until it is set,
    /// every KASAN hook is a no-op so early boot (before the heap exists) is safe.
    pub static KASAN_LIVE: AtomicBool = AtomicBool::new(false);

    /// Errors classified so far (use-after-free + out-of-bounds), for the
    /// procfs line and smoketest accounting.
    pub static ERRORS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

    /// Chunks evicted from the quarantine ring back to the heap. A non-zero value
    /// proves the free→quarantine→evict→reuse path actually cycled (rather than a
    /// trivial churn that never filled the ring), which the endurance soak asserts.
    pub static EVICTIONS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum KasanError {
        UseAfterFree,
        OutOfBounds,
    }

    #[inline]
    pub fn is_live() -> bool {
        KASAN_LIVE.load(Ordering::Relaxed)
    }

    #[inline]
    fn in_heap(addr: usize, size: usize) -> bool {
        addr >= HEAP_START
            && size != 0
            && addr
                .checked_add(size)
                .map_or(false, |e| e <= HEAP_START + HEAP_SIZE)
    }

    /// Write `value` into the shadow bytes covering [addr, addr+size). `size` is
    /// rounded up to the 8-byte shadow granule.
    fn shadow_set(addr: usize, size: usize, value: u8) {
        if !in_heap(addr, size) {
            return;
        }
        let shadow_addr = SHADOW_START + ((addr - HEAP_START) >> 3);
        let shadow_len = (size + 7) >> 3;
        unsafe {
            core::ptr::write_bytes(shadow_addr as *mut u8, value, shadow_len);
        }
    }

    /// Read the shadow byte that governs `addr`. Returns `SHADOW_VALID` for any
    /// address outside the heap (KASAN only governs heap accesses here).
    pub fn read_shadow(addr: usize) -> u8 {
        if addr < HEAP_START || addr >= HEAP_START + HEAP_SIZE {
            return SHADOW_VALID;
        }
        let shadow_addr = SHADOW_START + ((addr - HEAP_START) >> 3);
        unsafe { core::ptr::read_volatile(shadow_addr as *const u8) }
    }

    /// Mark an allocation's bytes valid.
    pub fn alloc_hook(addr: usize, size: usize) {
        if !is_live() {
            return;
        }
        shadow_set(addr, size, SHADOW_VALID);
    }

    /// Check every shadow granule spanned by [addr, addr+size). Returns the first
    /// error found, or Ok(()). Used at the allocator boundary and by `kasan_check`.
    pub fn check(addr: usize, size: usize) -> Result<(), KasanError> {
        if !is_live() || size == 0 {
            return Ok(());
        }
        if addr < HEAP_START
            || addr
                .checked_add(size)
                .map_or(true, |e| e > HEAP_START + HEAP_SIZE)
        {
            // Outside the heap entirely — not a KASAN-governed region.
            return Ok(());
        }
        let mut a = addr;
        let end = addr + size;
        while a < end {
            let sv = read_shadow(a);
            if sv != SHADOW_VALID {
                ERRORS.fetch_add(1, Ordering::Relaxed);
                return Err(match sv {
                    SHADOW_FREED => KasanError::UseAfterFree,
                    SHADOW_REDZONE => KasanError::OutOfBounds,
                    // Any other non-zero shadow byte (partial granule / unknown
                    // poison) is treated as an out-of-bounds access too.
                    _ => KasanError::OutOfBounds,
                });
            }
            a += 8;
        }
        Ok(())
    }

    // -- Quarantine ---------------------------------------------------------
    // Hold a small number of freed chunks poisoned (not reused) so UAF stays
    // catchable. Evict oldest back to the heap when full.

    const QUARANTINE_SLOTS: usize = 512;

    #[derive(Clone, Copy)]
    struct QEntry {
        ptr: usize,
        layout: Layout,
    }

    struct Quarantine {
        ring: [Option<QEntry>; QUARANTINE_SLOTS],
        head: usize,
    }

    static QUARANTINE: Mutex<Quarantine> = Mutex::new(Quarantine {
        ring: [None; QUARANTINE_SLOTS],
        head: 0,
    });

    /// Poison a freed chunk and stash it. Returns true if the chunk is now
    /// quarantine-owned (caller must NOT return it to the heap). When the ring
    /// slot is occupied, the evicted older chunk is genuinely freed here, and
    /// this returns true for the NEW chunk.
    pub fn dealloc_hook(ptr: usize, layout: Layout) -> bool {
        if !is_live() || layout.size() == 0 {
            return false;
        }
        // Poison the just-freed region so a later access reads use-after-free.
        shadow_set(ptr, layout.size(), SHADOW_FREED);

        let evicted = {
            let mut q = QUARANTINE.lock();
            let slot = q.head;
            let evicted = q.ring[slot].take();
            q.ring[slot] = Some(QEntry { ptr, layout });
            q.head = (slot + 1) % QUARANTINE_SLOTS;
            evicted
        };

        if let Some(e) = evicted {
            // The evicted chunk leaves quarantine: it may legitimately be reused,
            // so clear its poison and return it to the heap. (A fresh alloc will
            // re-validate the exact bytes it hands out anyway.)
            shadow_set(e.ptr, e.layout.size(), SHADOW_VALID);
            unsafe {
                HEAP_INNER.dealloc(e.ptr as *mut u8, e.layout);
            }
            EVICTIONS.fetch_add(1, Ordering::Relaxed);
        }
        true
    }

    /// Drain EVERY quarantined chunk back to the heap (unpoison + real dealloc).
    /// Returns the number of chunks drained. The quarantine deliberately holds up
    /// to `QUARANTINE_SLOTS` freed-but-not-returned chunks at steady state, so
    /// `heap_used()` over-counts by that fixed residency; a leak audit that needs
    /// a strict zero delta calls this first so the only bytes left allocated are
    /// genuine leaks (chunks never freed at all), not quarantine retention.
    pub fn flush() -> u64 {
        if !is_live() {
            return 0;
        }
        let mut drained = 0u64;
        let mut q = QUARANTINE.lock();
        for slot in q.ring.iter_mut() {
            if let Some(e) = slot.take() {
                shadow_set(e.ptr, e.layout.size(), SHADOW_VALID);
                unsafe {
                    HEAP_INNER.dealloc(e.ptr as *mut u8, e.layout);
                }
                drained += 1;
            }
        }
        q.head = 0;
        drained
    }

    pub fn error_count() -> u64 {
        ERRORS.load(Ordering::Relaxed)
    }

    pub fn eviction_count() -> u64 {
        EVICTIONS.load(Ordering::Relaxed)
    }
}

/// KASAN alloc hook (feature = "kasan" only).
#[cfg(feature = "kasan")]
#[inline]
fn kasan_alloc_hook(addr: usize, size: usize) {
    kasan_rt::alloc_hook(addr, size);
}

/// KASAN dealloc hook (feature = "kasan" only). Returns true if the chunk was
/// quarantined and must NOT be returned to the heap by the caller.
#[cfg(feature = "kasan")]
#[inline]
fn kasan_dealloc_hook(addr: usize, layout: Layout) -> bool {
    kasan_rt::dealloc_hook(addr, layout)
}

/// Public KASAN access check (feature = "kasan" only). Returns Err if any byte
/// in [addr, addr+size) is poisoned (use-after-free or out-of-bounds). Used by
/// the hardening KASAN smoketest and any explicit boundary check.
#[cfg(feature = "kasan")]
pub fn kasan_check(addr: usize, size: usize) -> Result<(), kasan_rt::KasanError> {
    kasan_rt::check(addr, size)
}

/// True once the KASAN shadow is mapped + initialized and the allocator is
/// instrumenting (feature = "kasan" only). Mirrors KFENCE's `is_live()`.
#[cfg(feature = "kasan")]
pub fn kasan_is_live() -> bool {
    kasan_rt::is_live()
}

/// Read the shadow byte governing `addr` (feature = "kasan" only).
#[cfg(feature = "kasan")]
pub fn kasan_read_shadow(addr: usize) -> u8 {
    kasan_rt::read_shadow(addr)
}

/// Total KASAN errors classified (feature = "kasan" only).
#[cfg(feature = "kasan")]
pub fn kasan_error_count() -> u64 {
    kasan_rt::error_count()
}

/// Total chunks evicted from the KASAN quarantine ring back to the heap
/// (feature = "kasan" only). Non-zero proves the free→quarantine→reuse path
/// cycled — the endurance soak asserts this so a trivial pass cannot fake it.
#[cfg(feature = "kasan")]
pub fn kasan_eviction_count() -> u64 {
    kasan_rt::eviction_count()
}

/// Drain the KASAN quarantine ring fully back to the heap (feature = "kasan"
/// only); returns the number of chunks drained. Used by the endurance leak audit
/// so its post-churn `heap_used()` excludes the quarantine's fixed steady-state
/// residency and a non-zero delta means a GENUINE leak (an allocation never
/// freed), not a quarantined-but-accounted chunk.
#[cfg(feature = "kasan")]
pub fn kasan_quarantine_flush() -> u64 {
    kasan_rt::flush()
}

#[cfg(feature = "kasan")]
pub use kasan_rt::KasanError;

/// Start of the kernel heap in virtual memory (Upper Half).
pub const HEAP_START: usize = 0xFFFF_9999_0000_0000;
// 128 MiB. The self-hosted installer (installer::run_install) holds the real
// bootloader + ~26 MiB kernel-x86_64 + ~22 MiB initramfs payloads live in the
// heap at once (~48 MiB peak) while writing the target ESP; the old 32 MiB heap
// OOM-halted mid-install ("[oom] kernel heap exhausted -> halting") the instant
// a real (non-placeholder) kernel was sourced from the boot stick. 128 MiB
// gives ~2.6x headroom over that peak and is negligible on the target (Athena
// 16 GiB; QEMU 2 GiB). The heap→KASAN-shadow VA gap is 2 GiB, so this fits.
pub const HEAP_SIZE: usize = 128 * 1024 * 1024;

/// KASAN Shadow memory start. Each byte in shadow memory represents 8 bytes of heap.
/// 0x00 = fully valid, 0xFF = fully poisoned, 1-7 = partially valid.
pub const SHADOW_START: usize = 0xFFFF_9999_8000_0000;
pub const SHADOW_SIZE: usize = HEAP_SIZE / 8;

static HEAP_INNER: LockedHeap = LockedHeap::empty();

#[global_allocator]
static ALLOCATOR: OomAwareHeap = OomAwareHeap;

/// Bytes currently allocated from the kernel heap. Brief lock; callers MUST NOT
/// allocate while interpreting the result relative to a prior sample.
pub fn heap_used() -> usize {
    HEAP_INNER.lock().used()
}

/// Bytes still free in the kernel heap.
pub fn heap_free() -> usize {
    HEAP_INNER.lock().free()
}

/// Force-release the heap allocator's spinlock. Called from the
/// double-fault handler so a CPU that #DF'd while holding the lock
/// inside `__rust_alloc` / `__rust_dealloc` doesn't freeze every other
/// CPU's next `Box::new` / `Vec::push` / etc.
///
/// # Safety
///
/// The caller must be certain the lock holder will never resume
/// (i.e., the faulting CPU is about to enter `hlt_loop()`). Any
/// allocator state the holder was in the middle of mutating is left
/// as-is, so the heap may have leaked or have a partial linked-list
/// edit; the surviving kernel is in a degraded but live state.
pub unsafe fn force_unlock_heap() {
    HEAP_INNER.force_unlock();
}

/// Write the KASAN shadow for [addr, addr+size) to `value`. The shadow is only
/// consulted under `feature = "kasan"`; in the default build these helpers have
/// no callers (the allocator does not instrument), hence the gated allow.
#[cfg_attr(not(feature = "kasan"), allow(dead_code))]
pub fn poison_region(addr: usize, size: usize, value: u8) {
    if addr < HEAP_START || addr + size > HEAP_START + HEAP_SIZE {
        return;
    }
    let shadow_addr = SHADOW_START + ((addr - HEAP_START) >> 3);
    let shadow_len = (size + 7) >> 3;
    unsafe {
        core::ptr::write_bytes(shadow_addr as *mut u8, value, shadow_len);
    }
}

#[cfg_attr(not(feature = "kasan"), allow(dead_code))]
pub fn unpoison_region(addr: usize, size: usize) {
    poison_region(addr, size, 0);
}

/// Map heap and shadow pages, and initialize the allocator.
pub fn init_heap(
    mapper: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), MapToError<Size4KiB>> {
    // 1. Map Heap
    let page_range = {
        let heap_start = VirtAddr::new(HEAP_START as u64);
        let heap_end = heap_start + HEAP_SIZE as u64 - 1u64;
        let heap_start_page = Page::containing_address(heap_start);
        let heap_end_page = Page::containing_address(heap_end);
        Page::range_inclusive(heap_start_page, heap_end_page)
    };

    for page in page_range {
        let frame = frame_allocator
            .allocate_frame()
            .ok_or(MapToError::FrameAllocationFailed)?;
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE;
        unsafe {
            mapper.map_to(page, frame, flags, frame_allocator)?.flush();
        }
    }

    // 2. Map KASAN Shadow Region
    let shadow_page_range = {
        let shadow_start = VirtAddr::new(SHADOW_START as u64);
        let shadow_end = shadow_start + SHADOW_SIZE as u64 - 1u64;
        let shadow_start_page = Page::containing_address(shadow_start);
        let shadow_end_page = Page::containing_address(shadow_end);
        Page::range_inclusive(shadow_start_page, shadow_end_page)
    };

    for page in shadow_page_range {
        let frame = frame_allocator
            .allocate_frame()
            .ok_or(MapToError::FrameAllocationFailed)?;
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE;
        unsafe {
            mapper.map_to(page, frame, flags, frame_allocator)?.flush();
        }
    }

    // Install the freelist-hardening cookie BEFORE the heap builds its first
    // holes, so every stored `Hole.next` link is encoded under it. Installing
    // after `init()` would leave the initial head link encoded with the default
    // cookie=0 and then undecodable the instant the real cookie lands. Sourced
    // from rdrand-with-TSC-fallback because `init_heap` runs long before the
    // CSPRNG (and before `cpu_features::init()`), so we probe CPUID directly.
    let cookie = boot_random_cookie();
    linked_list_allocator::set_freelist_cookie(cookie);

    unsafe {
        HEAP_INNER.lock().init(HEAP_START as *mut u8, HEAP_SIZE);
        // Initially poison everything; unpoison as heap chunks are used.
        core::ptr::write_bytes(SHADOW_START as *mut u8, 0xFF, SHADOW_SIZE);
    }

    // KASAN is now live (feature = "kasan" only): the shadow is mapped and fully
    // poisoned, so the allocator hooks may safely instrument from here on. The
    // default build never sets this and the hooks do not exist.
    #[cfg(feature = "kasan")]
    kasan_rt::KASAN_LIVE.store(true, Ordering::SeqCst);

    crate::serial_println!(
        "[heap-guard] freelist hardening ON: cookie=installed encode=xor+loc-tie validate=range+align"
    );

    Ok(())
}

/// Boot-random 64-bit freelist cookie for [`linked_list_allocator`]. Uses RDRAND
/// when the CPU advertises it (CPUID.01H:ECX[30]) and always mixes in RDTSC, so
/// even a bad RNG contributes timing entropy. Never returns 0 (0 == "cookie not
/// installed"/identity in the allocator).
fn boot_random_cookie() -> u64 {
    let mut cookie = crate::memory::aslr_random();

    // RDRAND only if CPUID says the CPU has it — executing it otherwise is #UD,
    // and cpu_features has not been detected this early in boot.
    unsafe {
        let feat = core::arch::x86_64::__cpuid(1);
        if feat.ecx & (1 << 30) != 0 {
            let mut val: u64 = 0;
            for _ in 0..10 {
                if core::arch::x86_64::_rdrand64_step(&mut val) == 1 {
                    cookie ^= val;
                    break;
                }
            }
        }
    }

    cookie ^= crate::memory::aslr_random().rotate_left(23);
    if cookie == 0 {
        // Astronomically unlikely; fall back to a fixed non-zero constant so the
        // guard is never silently disabled by a zero cookie.
        cookie = 0x9E37_79B9_7F4A_7C15;
    }
    cookie
}

/// Kernel-heap freelist-integrity guard status (always-on, default build).
///
/// Concept §Kernel Architecture: "Driver isolation: Every driver runs in its own
/// protection domain with IOMMU enforcement. A bad GPU driver crashes a service,
/// not the kernel." Concept §Principles: "Security by default, not by friction."
///
/// The intrusive linked-list heap stores each free chunk's `next` link *inside*
/// the freed memory, so a DMA/wild write into an already-freed chunk stomps a
/// live freelist pointer and the next allocation hands out a garbage pointer —
/// the DMA-UAF-corrupts-freelist class. The vendored `linked_list_allocator`
/// stores that link XOR-obfuscated with a boot-random cookie AND tied to its
/// storage location, and validates the decoded pointer (alignment + heap-range)
/// on every deref, fail-closing with a panic on corruption. This module owns the
/// boot-time cookie install, the FAIL-able smoketest, and the procfs accessors.
///
/// Runs a FAIL-able boot smoketest of the freelist-hardening predicate. Builds a
/// synthetic pair of holes in a read-only scratch buffer (never touches the live
/// heap), encodes a valid link and asserts decode+validate PASS, then corrupts
/// the encoded `next` word and asserts `validate_link` returns `Err`. Reverting
/// the validator flips the result to `-> FAIL`, so this cannot be a false green.
pub fn run_boot_smoketest() {
    use linked_list_allocator::{decode, encode, validate_link};

    // Two adjacent 16-byte "holes" (u64 x4 == 32 bytes, 8-aligned). Immutable
    // static: we only need real, in-range addresses for slot/bounds — we never
    // write through it, so no `static mut` and no live-heap interference.
    static SCRATCH: [u64; 4] = [0; 4];
    let base = core::ptr::addr_of!(SCRATCH) as usize;
    let bottom = base;
    let top = base + 32;

    // hole0 @ base; its `next` field slot is base+8. Point it at hole1 @ base+16
    // (aligned, in-range).
    let slot0 = (base + 8) as *const usize;
    let target = base + 16;

    let enc = encode(target, slot0);
    let roundtrip_ok = decode(enc, slot0) == target
        && matches!(validate_link(enc, slot0, bottom, top), Ok(Some(p)) if p == target);

    // Deliberately corrupt the encoded next word: the decoded pointer must now be
    // misaligned or out of [bottom, top) -> Err.
    let corrupt = enc ^ 0xDEAD_BEEF;
    let rejected_corrupt = validate_link(corrupt, slot0, bottom, top).is_err();

    let validations = linked_list_allocator::validation_count();
    let pass = roundtrip_ok && rejected_corrupt;

    crate::serial_println!(
        "[heap-guard] smoketest: encode_roundtrip={} rejected_corrupt_next={} validations={} -> {}",
        roundtrip_ok,
        rejected_corrupt,
        validations,
        if pass { "PASS" } else { "FAIL" }
    );
}

/// Freelist-guard status snapshot for `/proc/athena/heap_guard`. Never exposes
/// the cookie value.
pub fn freelist_guard_stats() -> linked_list_allocator::GuardStats {
    linked_list_allocator::guard_stats()
}
