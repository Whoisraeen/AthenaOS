use core::alloc::{Layout, LayoutError};
use core::mem;
use core::mem::{align_of, size_of};
use core::ptr::null_mut;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::{align_down_size, align_up_size};

use super::align_up;

// ===========================================================================
// RaeenOS freelist hardening (always-on; MIT/Apache — RaeenOS addition).
//
// Concept promise served:
//   "Driver isolation: Every driver runs in its own protection domain with
//    IOMMU enforcement. A bad GPU driver crashes a service, not the kernel."
//    (RaeenOS_Concept.md §Kernel Architecture)
//   "Security by default, not by friction." (§Principles)
//
// The intrusive free list stores each free chunk's `next` link *inside* the
// freed memory. A DMA/wild write into an already-freed chunk therefore stomps a
// live freelist pointer, and the next allocation that follows it hands out an
// attacker/garbage pointer. We defend that exact class by storing `next`
// XOR-obfuscated with a boot-random cookie AND tied to the storage location
// (the `swab(&location)` term of Linux CONFIG_SLAB_FREELIST_HARDENED): a wild
// data write cannot forge a valid-looking link without knowing the cookie, and
// a link copied to a different slot decodes to garbage. Every deref validates
// the decoded pointer's alignment + heap-range and, on the live path,
// fail-closes with `panic_corruption` (a corrupt intrusive list has no
// authoritative side-table to recover from, unlike buddy.rs's per-frame bitmap).
// ===========================================================================

/// Boot-random per-heap cookie. `0` means "not installed" — encode/decode still
/// round-trip (the location term alone keeps the list walkable), so a heap that
/// never calls [`set_freelist_cookie`] (e.g. the raeen_linuxkpi shim) is simply
/// unhardened rather than broken.
static FREELIST_COOKIE: AtomicU64 = AtomicU64::new(0);
static COOKIE_INSTALLED: AtomicBool = AtomicBool::new(false);
static VALIDATIONS: AtomicU64 = AtomicU64::new(0);
static CORRUPTIONS: AtomicU64 = AtomicU64::new(0);
/// Decoded address of the most recent corruption (0 = none). Only ever written
/// by `panic_corruption`, so on a healthy system it stays 0.
static LAST_CORRUPTION: AtomicU64 = AtomicU64::new(0);

/// Kinds of freelist-link corruption the validator can detect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeapCorruption {
    /// Decoded pointer is not `align_of::<Hole>()`-aligned.
    Misaligned,
    /// Decoded pointer does not land inside `[bottom, top)`.
    OutOfRange,
}

impl HeapCorruption {
    /// Stable lower-case label for the `[heap] CORRUPTION …` line and procfs.
    pub const fn label(self) -> &'static str {
        match self {
            HeapCorruption::Misaligned => "misaligned",
            HeapCorruption::OutOfRange => "out-of-range",
        }
    }
}

/// Snapshot of the freelist-guard counters for `/proc/raeen/heap_guard`.
pub struct GuardStats {
    pub cookie_installed: bool,
    pub validations: u64,
    pub corruptions: u64,
    /// Decoded address of the last detected corruption, if any.
    pub last_corruption: Option<usize>,
}

/// Install the boot-random freelist cookie for this heap. Call once, *before*
/// the heap builds its first holes, so every stored link is encoded with it.
/// A value of `0` disables obfuscation (leaves only the location tie).
pub fn set_freelist_cookie(cookie: u64) {
    FREELIST_COOKIE.store(cookie, Ordering::SeqCst);
    COOKIE_INSTALLED.store(true, Ordering::SeqCst);
}

/// True once [`set_freelist_cookie`] has run.
pub fn cookie_installed() -> bool {
    COOKIE_INSTALLED.load(Ordering::Relaxed)
}

/// Total freelist-link validations performed since boot.
pub fn validation_count() -> u64 {
    VALIDATIONS.load(Ordering::Relaxed)
}

/// Total freelist-link corruptions detected since boot (each one panics the
/// live path, so a running system reads `0`).
pub fn corruption_count() -> u64 {
    CORRUPTIONS.load(Ordering::Relaxed)
}

/// Consolidated snapshot for procfs. Never exposes the cookie value.
pub fn guard_stats() -> GuardStats {
    let corruptions = CORRUPTIONS.load(Ordering::Relaxed);
    let last = LAST_CORRUPTION.load(Ordering::Relaxed);
    GuardStats {
        cookie_installed: COOKIE_INSTALLED.load(Ordering::Relaxed),
        validations: VALIDATIONS.load(Ordering::Relaxed),
        corruptions,
        last_corruption: if last != 0 { Some(last as usize) } else { None },
    }
}

#[inline]
fn cookie() -> usize {
    FREELIST_COOKIE.load(Ordering::Relaxed) as usize
}

/// Location-tied XOR encode (mirrors SLUB `freelist_ptr_encode`). `slot` is the
/// address the ciphertext lives at, so the same pointer stored at a different
/// slot encodes to a different word. `0` (list tail / `None`) is stored as `0`
/// verbatim — a wild write that zeroes a link therefore only truncates the list
/// (a leak, not a wild-pointer follow), matching glibc safe-linking's NULL fd.
#[inline]
pub fn encode(ptr: usize, slot: *const usize) -> usize {
    if ptr == 0 {
        0
    } else {
        ptr ^ cookie() ^ (slot as usize).rotate_left(32)
    }
}

/// Inverse of [`encode`]. Symmetric: `decode(encode(p, s), s) == p` for all `p`.
#[inline]
pub fn decode(enc: usize, slot: *const usize) -> usize {
    if enc == 0 {
        0
    } else {
        enc ^ cookie() ^ (slot as usize).rotate_left(32)
    }
}

/// The whole detector as a **pure predicate** — no panic, so the host KAT and
/// the boot smoketest can feed it deliberately-corrupted input and observe
/// `Err`. The live allocator wraps this in `panic_corruption` (fail-closed).
///
/// Returns the decoded target address on success (`Ok(Some(addr))`), `Ok(None)`
/// for the list tail, or `Err` if the decoded link is misaligned / out of the
/// heap span `[bottom, top)`.
pub fn validate_link(
    enc: usize,
    slot: *const usize,
    bottom: usize,
    top: usize,
) -> Result<Option<usize>, HeapCorruption> {
    // Diagnostic counter only. This runs on EVERY freelist-link deref (the
    // allocator's hottest inner loop — ~800k times during boot), and it is
    // ALWAYS reached under the `LockedHeap` spinlock, so it can never race.
    // A `fetch_add` (`lock xadd`) is therefore pointless here AND expensive
    // under QEMU TCG's atomic emulation (measured ~900 ms of boot on the
    // full-devset CI boot); a plain relaxed load+store is a couple of cheap
    // `mov`s and keeps the exact telemetry. (Aligned u64 loads by a concurrent
    // procfs reader stay tear-free on x86-64 — a slightly stale count is fine.)
    VALIDATIONS.store(VALIDATIONS.load(Ordering::Relaxed) + 1, Ordering::Relaxed);
    validate_decoded(decode(enc, slot), bottom, top)
}

/// The range/alignment predicate over an already-decoded link. Single source of
/// truth shared by the live [`validate_link`] and the test-only helpers.
#[inline]
fn validate_decoded(p: usize, bottom: usize, top: usize) -> Result<Option<usize>, HeapCorruption> {
    if p == 0 {
        return Ok(None);
    }
    if p % align_of::<Hole>() != 0 {
        return Err(HeapCorruption::Misaligned);
    }
    if p < bottom || p.checked_add(size_of::<Hole>()).map_or(true, |end| end > top) {
        return Err(HeapCorruption::OutOfRange);
    }
    Ok(Some(p))
}

/// Test-only pure encode with an explicit cookie (no global state), so the host
/// KAT can exercise many cookie values without racing the shared
/// `FREELIST_COOKIE` across parallel test threads.
#[cfg(test)]
pub fn encode_with(ptr: usize, slot: *const usize, cookie: u64) -> usize {
    if ptr == 0 {
        0
    } else {
        ptr ^ cookie as usize ^ (slot as usize).rotate_left(32)
    }
}

/// Test-only inverse of [`encode_with`].
#[cfg(test)]
pub fn decode_with(enc: usize, slot: *const usize, cookie: u64) -> usize {
    if enc == 0 {
        0
    } else {
        enc ^ cookie as usize ^ (slot as usize).rotate_left(32)
    }
}

/// Test-only validate with an explicit cookie (race-free), delegating to the
/// same predicate as the live path.
#[cfg(test)]
pub fn validate_with(
    enc: usize,
    slot: *const usize,
    cookie: u64,
    bottom: usize,
    top: usize,
) -> Result<Option<usize>, HeapCorruption> {
    validate_decoded(decode_with(enc, slot, cookie), bottom, top)
}

/// Fail-closed handler for a corrupt link on the live allocator path. Records
/// the event and panics with a dedicated, greppable line. A detected
/// heap-metadata corruption is non-recoverable (continuing risks handing out a
/// wild pointer), matching Windows `KeBugCheck` on pool-metadata corruption.
#[cold]
#[inline(never)]
fn panic_corruption(err: HeapCorruption, enc: usize, slot: *const usize) -> ! {
    CORRUPTIONS.fetch_add(1, Ordering::SeqCst);
    let decoded = decode(enc, slot);
    LAST_CORRUPTION.store(decoded as u64, Ordering::SeqCst);
    panic!(
        "[heap] CORRUPTION: hole@{:#x} enc={:#x} decoded={:#x} ({}) -> PANIC",
        slot as usize,
        enc,
        decoded,
        err.label(),
    );
}

/// A sorted list of holes. It uses the the holes itself to store its nodes.
pub struct HoleList {
    pub(crate) first: Hole, // dummy
    pub(crate) bottom: *mut u8,
    pub(crate) top: *mut u8,
    pub(crate) pending_extend: u8,
}

pub(crate) struct Cursor {
    prev: NonNull<Hole>,
    hole: NonNull<Hole>,
    top: *mut u8,
    bottom: *mut u8,
    /// True while `prev` is the `HoleList::first` dummy head. The head lives in
    /// trusted (non-heap) memory and its link is encoded with the move-invariant
    /// head slot (null) rather than its own address; every real hole uses its
    /// own address as the slot. This flag routes writes to `prev.next` through
    /// the correct slot.
    prev_is_head: bool,
}

/// A block containing free memory. It points to the next hole and thus forms a linked list.
///
/// On-disk layout is unchanged (16 bytes `{ size, next_enc }`); the only
/// difference from upstream is that `next_enc` stores the XOR-obfuscated,
/// location-tied encoding of the link rather than a raw pointer. All access
/// goes through [`Hole::next_checked`] / [`Hole::store_next`].
pub(crate) struct Hole {
    pub size: usize,
    /// Encoded `next` link (see [`encode`]). `0` == list tail (`None`).
    pub next_enc: usize,
}

impl Hole {
    /// The address this hole's encoded link is stored at. Real holes tie their
    /// encoding to this; the dummy head passes `null` instead (move-invariant).
    #[inline]
    fn real_slot(&self) -> *const usize {
        core::ptr::addr_of!(self.next_enc)
    }

    #[inline]
    fn slot(&self, is_head: bool) -> *const usize {
        if is_head {
            core::ptr::null()
        } else {
            self.real_slot()
        }
    }

    /// Validated decode of this hole's `next` link (pure — never panics).
    #[inline]
    fn load_next(
        &self,
        is_head: bool,
        bottom: usize,
        top: usize,
    ) -> Result<Option<NonNull<Hole>>, HeapCorruption> {
        let decoded = validate_link(self.next_enc, self.slot(is_head), bottom, top)?;
        // SAFETY: `validate_link` guarantees `addr` is non-zero, aligned and
        // in-range, so the pointer is valid to form.
        Ok(decoded.map(|addr| unsafe { NonNull::new_unchecked(addr as *mut Hole) }))
    }

    /// Validated decode on the **live** path: fail-closed panic on corruption.
    #[inline]
    fn next_checked(&self, is_head: bool, bottom: usize, top: usize) -> Option<NonNull<Hole>> {
        match self.load_next(is_head, bottom, top) {
            Ok(v) => v,
            Err(e) => panic_corruption(e, self.next_enc, self.slot(is_head)),
        }
    }

    /// Encode + store `val` at this hole's slot.
    #[inline]
    fn store_next(&mut self, is_head: bool, val: Option<NonNull<Hole>>) {
        let ptr = val.map_or(0, |n| n.as_ptr() as usize);
        self.next_enc = encode(ptr, self.slot(is_head));
    }

    /// Test-only unchecked decode (no bounds), for the vendored crate's own
    /// regression tests that inspect list structure directly.
    #[cfg(test)]
    pub fn decoded_next(&self, is_head: bool) -> Option<NonNull<Hole>> {
        let p = decode(self.next_enc, self.slot(is_head));
        NonNull::new(p as *mut Hole)
    }
}

/// Basic information about a hole.
#[derive(Debug, Clone, Copy)]
struct HoleInfo {
    addr: *mut u8,
    size: usize,
}

impl Cursor {
    fn next(self) -> Option<Self> {
        let bottom = self.bottom as usize;
        let top = self.top as usize;
        // `self.hole` is a real hole (never the head), so decode with is_head=false.
        let nhole = unsafe { self.hole.as_ref() }.next_checked(false, bottom, top);
        nhole.map(|nhole| Cursor {
            prev: self.hole,
            hole: nhole,
            top: self.top,
            bottom: self.bottom,
            // After advancing, `prev` is the old current — always a real hole.
            prev_is_head: false,
        })
    }

    fn current(&self) -> &Hole {
        unsafe { self.hole.as_ref() }
    }

    fn previous(&self) -> &Hole {
        unsafe { self.prev.as_ref() }
    }

    // On success, it returns the new allocation, and the linked list has been updated
    // to accomodate any new holes and allocation. On error, it returns the cursor
    // unmodified, and has made no changes to the linked list of holes.
    fn split_current(self, required_layout: Layout) -> Result<(*mut u8, usize), Self> {
        let front_padding;
        let alloc_ptr;
        let alloc_size;
        let back_padding;

        // Capture guard context before the late destructure of `self` below.
        let prev_is_head = self.prev_is_head;
        let bottom = self.bottom as usize;
        let top = self.top as usize;

        // Here we create a scope, JUST to make sure that any created references do not
        // live to the point where we start doing pointer surgery below.
        {
            let hole_size = self.current().size;
            let hole_addr_u8 = self.hole.as_ptr().cast::<u8>();
            let required_size = required_layout.size();
            let required_align = required_layout.align();

            // Quick check: If the new item is larger than the current hole, it's never gunna
            // work. Go ahead and bail early to save ourselves some math.
            if hole_size < required_size {
                return Err(self);
            }

            // Attempt to fracture the current hole into the following parts:
            // ([front_padding], allocation, [back_padding])
            //
            // The paddings are optional, and only placed if required.
            //
            // First, figure out if front padding is necessary. This would be necessary if the new
            // allocation has a larger alignment requirement than the current hole, and we didn't get
            // lucky that the current position was well-aligned enough for the new item.
            let aligned_addr = if hole_addr_u8 == align_up(hole_addr_u8, required_align) {
                // hole has already the required alignment, no front padding is needed.
                front_padding = None;
                hole_addr_u8
            } else {
                // Unfortunately, we did not get lucky. Instead: Push the "starting location" FORWARD the size
                // of a hole node, to guarantee there is at least enough room for the hole header, and
                // potentially additional space.
                let new_start = hole_addr_u8.wrapping_add(HoleList::min_size());

                let aligned_addr = align_up(new_start, required_align);
                front_padding = Some(HoleInfo {
                    // Our new front padding will exist at the same location as the previous hole,
                    // it will just have a smaller size after we have chopped off the "tail" for
                    // the allocation.
                    addr: hole_addr_u8,
                    size: (aligned_addr as usize) - (hole_addr_u8 as usize),
                });
                aligned_addr
            };

            // Okay, now that we found space, we need to see if the decisions we just made
            // ACTUALLY fit in the previous hole space
            let allocation_end = aligned_addr.wrapping_add(required_size);
            let hole_end = hole_addr_u8.wrapping_add(hole_size);

            if allocation_end > hole_end {
                // hole is too small
                return Err(self);
            }

            // Yes! We have successfully placed our allocation as well.
            alloc_ptr = aligned_addr;
            alloc_size = required_size;

            // Okay, time to move onto the back padding.
            let back_padding_size = hole_end as usize - allocation_end as usize;
            back_padding = if back_padding_size == 0 {
                None
            } else {
                // NOTE: Because we always use `HoleList::align_layout`, the size of
                // the new allocation is always "rounded up" to cover any partial gaps that
                // would have occurred. For this reason, we DON'T need to "round up"
                // to account for an unaligned hole spot.
                let hole_layout = Layout::new::<Hole>();
                let back_padding_start = align_up(allocation_end, hole_layout.align());
                let back_padding_end = back_padding_start.wrapping_add(hole_layout.size());

                // Will the proposed new back padding actually fit in the old hole slot?
                if back_padding_end <= hole_end {
                    // Yes, it does! Place a back padding node
                    Some(HoleInfo {
                        addr: back_padding_start,
                        size: back_padding_size,
                    })
                } else {
                    // No, it does not. We don't want to leak any heap bytes, so we
                    // consider this hole unsuitable for the requested allocation.
                    return Err(self);
                }
            };
        }

        ////////////////////////////////////////////////////////////////////////////
        // This is where we actually perform surgery on the linked list.
        ////////////////////////////////////////////////////////////////////////////
        let Cursor {
            mut prev, mut hole, ..
        } = self;
        // Take the (validated) next node out of our current node.
        let maybe_next_addr: Option<NonNull<Hole>> = unsafe {
            let n = hole.as_ref().next_checked(false, bottom, top);
            hole.as_mut().store_next(false, None);
            n
        };

        // As of now, the old `Hole` is no more. We are about to replace it with one or more of
        // the front padding, the allocation, and the back padding.

        match (front_padding, back_padding) {
            (None, None) => {
                // No padding at all, how lucky! We still need to connect the PREVIOUS node
                // to the NEXT node, if there was one
                unsafe {
                    prev.as_mut().store_next(prev_is_head, maybe_next_addr);
                }
            }
            (None, Some(singlepad)) | (Some(singlepad), None) => unsafe {
                // We have front padding OR back padding, but not both.
                //
                // Replace the old node with the new single node. We need to stitch the new node
                // into the linked list. Start by writing the padding into the proper location.
                // The link is encoded at the pad's FINAL slot (`store_next` after `write`).
                let singlepad_ptr = singlepad.addr.cast::<Hole>();
                singlepad_ptr.write(Hole {
                    size: singlepad.size,
                    next_enc: 0,
                });
                // If the old hole had a next pointer, the single padding now takes
                // "ownership" of that link.
                (*singlepad_ptr).store_next(false, maybe_next_addr);

                // Then connect the OLD previous to the NEW single padding
                prev.as_mut()
                    .store_next(prev_is_head, Some(NonNull::new_unchecked(singlepad_ptr)));
            },
            (Some(frontpad), Some(backpad)) => unsafe {
                // We have front padding AND back padding.
                //
                // We need to stich them together as two nodes where there used to
                // only be one. Start with the back padding.
                let backpad_ptr = backpad.addr.cast::<Hole>();
                backpad_ptr.write(Hole {
                    size: backpad.size,
                    next_enc: 0,
                });
                // If the old hole had a next pointer, the BACK padding now takes
                // "ownership" of that link.
                (*backpad_ptr).store_next(false, maybe_next_addr);

                // Now we emplace the front padding, and link it to both the back padding,
                // and the old previous
                let frontpad_ptr = frontpad.addr.cast::<Hole>();
                frontpad_ptr.write(Hole {
                    size: frontpad.size,
                    next_enc: 0,
                });
                // We now connect the FRONT padding to the BACK padding.
                (*frontpad_ptr).store_next(false, Some(NonNull::new_unchecked(backpad_ptr)));

                // Then connect the OLD previous to the NEW FRONT padding
                prev.as_mut()
                    .store_next(prev_is_head, Some(NonNull::new_unchecked(frontpad_ptr)));
            },
        }

        // Well that went swimmingly! Hand off the allocation, with surgery performed successfully!
        Ok((alloc_ptr, alloc_size))
    }
}

// See if we can extend this hole towards the end of the allocation region
// If so: increase the size of the node. If no: keep the node as-is
fn check_merge_top(mut node: NonNull<Hole>, top: *mut u8) {
    let node_u8 = node.as_ptr().cast::<u8>();
    let node_sz = unsafe { node.as_ref().size };

    // If this is the last node, we need to see if we need to merge to the end
    let end = node_u8.wrapping_add(node_sz);
    let hole_layout = Layout::new::<Hole>();
    if end < top {
        let next_hole_end = align_up(end, hole_layout.align()).wrapping_add(hole_layout.size());

        if next_hole_end > top {
            let offset = (top as usize) - (end as usize);
            unsafe {
                node.as_mut().size += offset;
            }
        }
    }
}

// See if we can scoot this hole back to the bottom of the allocation region
// If so: create and return the new hole. If not: return the existing hole
fn check_merge_bottom(node: NonNull<Hole>, bottom: *mut u8) -> NonNull<Hole> {
    debug_assert_eq!(bottom as usize % align_of::<Hole>(), 0);

    if bottom.wrapping_add(core::mem::size_of::<Hole>()) > node.as_ptr().cast::<u8>() {
        let offset = (node.as_ptr() as usize) - (bottom as usize);
        let size = unsafe { node.as_ref() }.size + offset;
        unsafe { make_hole(bottom, size) }
    } else {
        node
    }
}

impl HoleList {
    /// Creates an empty `HoleList`.
    pub const fn empty() -> HoleList {
        HoleList {
            first: Hole {
                size: 0,
                // Encoded `None`: `decode(0, _) == 0`, so the empty head is valid
                // before any cookie is installed and before `new`/`init` runs.
                next_enc: 0,
            },
            bottom: null_mut(),
            top: null_mut(),
            pending_extend: 0,
        }
    }

    pub(crate) fn cursor(&mut self) -> Option<Cursor> {
        let bottom = self.bottom as usize;
        let top = self.top as usize;
        // The dummy head's link is encoded with the head slot (is_head=true).
        if let Some(hole) = self.first.next_checked(true, bottom, top) {
            Some(Cursor {
                hole,
                prev: NonNull::new(&mut self.first)?,
                top: self.top,
                bottom: self.bottom,
                prev_is_head: true,
            })
        } else {
            None
        }
    }

    #[cfg(any(test, fuzzing))]
    #[allow(dead_code)]
    pub(crate) fn debug(&mut self) {
        if let Some(cursor) = self.cursor() {
            let mut cursor = cursor;
            loop {
                println!(
                    "prev: {:?}[{}], hole: {:?}[{}]",
                    cursor.previous() as *const Hole,
                    cursor.previous().size,
                    cursor.current() as *const Hole,
                    cursor.current().size,
                );
                if let Some(c) = cursor.next() {
                    cursor = c;
                } else {
                    println!("Done!");
                    return;
                }
            }
        } else {
            println!("No holes");
        }
    }

    /// Creates a `HoleList` that contains the given hole.
    ///
    /// The `hole_addr` pointer is automatically aligned, so the `bottom`
    /// field might be larger than the given `hole_addr`.
    ///
    /// The given `hole_size` must be large enough to store the required
    /// metadata, otherwise this function will panic. Depending on the
    /// alignment of the `hole_addr` pointer, the minimum size is between
    /// `2 * size_of::<usize>` and `3 * size_of::<usize>`.
    ///
    /// The usable size for allocations will be truncated to the nearest
    /// alignment of `align_of::<usize>`. Any extra bytes left at the end
    /// will be reclaimed once sufficient additional space is given to
    /// [`extend`][crate::Heap::extend].
    ///
    /// # Safety
    ///
    /// This function is unsafe because it creates a hole at the given `hole_addr`.
    /// This can cause undefined behavior if this address is invalid or if memory from the
    /// `[hole_addr, hole_addr+size)` range is used somewhere else.
    pub unsafe fn new(hole_addr: *mut u8, hole_size: usize) -> HoleList {
        assert_eq!(size_of::<Hole>(), Self::min_size());
        assert!(hole_size >= size_of::<Hole>());

        let aligned_hole_addr = align_up(hole_addr, align_of::<Hole>());
        let requested_hole_size = hole_size - ((aligned_hole_addr as usize) - (hole_addr as usize));
        let aligned_hole_size = align_down_size(requested_hole_size, align_of::<Hole>());
        assert!(aligned_hole_size >= size_of::<Hole>());

        let ptr = aligned_hole_addr as *mut Hole;
        // Real hole, `next == None` (encoded `0` at its own slot).
        ptr.write(Hole {
            size: aligned_hole_size,
            next_enc: 0,
        });

        assert_eq!(
            hole_addr.wrapping_add(hole_size),
            aligned_hole_addr.wrapping_add(requested_hole_size)
        );

        let mut list = HoleList {
            first: Hole {
                size: 0,
                next_enc: 0,
            },
            bottom: aligned_hole_addr,
            top: aligned_hole_addr.wrapping_add(aligned_hole_size),
            pending_extend: (requested_hole_size - aligned_hole_size) as u8,
        };
        // Encode the head link with the move-invariant head slot (is_head=true),
        // so returning `list` by value / moving it into place stays valid.
        list.first
            .store_next(true, Some(NonNull::new_unchecked(ptr)));
        list
    }

    /// Aligns the given layout for use with `HoleList`.
    ///
    /// Returns a layout with size increased to fit at least `HoleList::min_size` and proper
    /// alignment of a `Hole`.
    ///
    /// The [`allocate_first_fit`][HoleList::allocate_first_fit] and
    /// [`deallocate`][HoleList::deallocate] methods perform the required alignment
    /// themselves, so calling this function manually is not necessary.
    pub fn align_layout(layout: Layout) -> Result<Layout, LayoutError> {
        let mut size = layout.size();
        if size < Self::min_size() {
            size = Self::min_size();
        }
        let size = align_up_size(size, mem::align_of::<Hole>());
        Layout::from_size_align(size, layout.align())
    }

    /// Searches the list for a big enough hole.
    ///
    /// A hole is big enough if it can hold an allocation of `layout.size()` bytes with
    /// the given `layout.align()`. If such a hole is found in the list, a block of the
    /// required size is allocated from it. Then the start address of that
    /// block and the aligned layout are returned. The automatic layout alignment is required
    /// because the `HoleList` has some additional layout requirements for each memory block.
    ///
    /// This function uses the “first fit” strategy, so it uses the first hole that is big
    /// enough. Thus the runtime is in O(n) but it should be reasonably fast for small allocations.
    //
    // NOTE: We could probably replace this with an `Option` instead of a `Result` in a later
    // release to remove this clippy warning
    #[allow(clippy::result_unit_err)]
    pub fn allocate_first_fit(&mut self, layout: Layout) -> Result<(NonNull<u8>, Layout), ()> {
        let aligned_layout = Self::align_layout(layout).map_err(|_| ())?;
        let mut cursor = self.cursor().ok_or(())?;

        loop {
            match cursor.split_current(aligned_layout) {
                Ok((ptr, _len)) => {
                    return Ok((NonNull::new(ptr).ok_or(())?, aligned_layout));
                }
                Err(curs) => {
                    cursor = curs.next().ok_or(())?;
                }
            }
        }
    }

    /// Frees the allocation given by `ptr` and `layout`.
    ///
    /// This function walks the list and inserts the given block at the correct place. If the freed
    /// block is adjacent to another free block, the blocks are merged again.
    /// This operation is in `O(n)` since the list needs to be sorted by address.
    ///
    /// [`allocate_first_fit`]: HoleList::allocate_first_fit
    ///
    /// # Safety
    ///
    /// `ptr` must be a pointer returned by a call to the [`allocate_first_fit`] function with
    /// identical layout. Undefined behavior may occur for invalid arguments.
    /// The function performs exactly the same layout adjustments as [`allocate_first_fit`] and
    /// returns the aligned layout.
    pub unsafe fn deallocate(&mut self, ptr: NonNull<u8>, layout: Layout) -> Layout {
        let aligned_layout = Self::align_layout(layout).unwrap();
        deallocate(self, ptr.as_ptr(), aligned_layout.size());
        aligned_layout
    }

    /// Returns the minimal allocation size. Smaller allocations or deallocations are not allowed.
    pub fn min_size() -> usize {
        size_of::<usize>() * 2
    }

    /// Returns information about the first hole for test purposes.
    #[cfg(test)]
    pub fn first_hole(&self) -> Option<(*const u8, usize)> {
        self.first.decoded_next(true).map(|hole| {
            (hole.as_ptr() as *mut u8 as *const u8, unsafe {
                hole.as_ref().size
            })
        })
    }

    /// Test-only: the decoded head link (the first real hole), if any.
    #[cfg(test)]
    pub fn head_next(&self) -> Option<NonNull<Hole>> {
        self.first.decoded_next(true)
    }

    pub(crate) unsafe fn extend(&mut self, by: usize) {
        assert!(!self.top.is_null(), "tried to extend an empty heap");

        let top = self.top;

        let dead_space = top.align_offset(align_of::<Hole>());
        debug_assert_eq!(
            0, dead_space,
            "dead space detected during extend: {} bytes. This means top was unaligned",
            dead_space
        );

        debug_assert!(
            (self.pending_extend as usize) < Self::min_size(),
            "pending extend was larger than expected"
        );

        // join this extend request with any pending (but not yet acted on) extension
        let extend_by = self.pending_extend as usize + by;

        let minimum_extend = Self::min_size();
        if extend_by < minimum_extend {
            self.pending_extend = extend_by as u8;
            return;
        }

        // only extend up to another valid boundary
        let new_hole_size = align_down_size(extend_by, align_of::<Hole>());
        let layout = Layout::from_size_align(new_hole_size, 1).unwrap();

        // Publish the new `top` BEFORE building the hole at the old top. RaeenOS
        // freelist hardening validates every decoded link against `[bottom, top)`,
        // and the hole we are about to instantiate lives at the old `top`; with
        // `top` still un-bumped its address would (correctly) read as out-of-range
        // and trip the guard. The memory is already owned here, so advancing `top`
        // first is sound and keeps the range check exact. (Upstream bumped `top`
        // after deallocate, which was fine only because upstream never validated.)
        let new_top = top.add(new_hole_size);
        self.top = new_top;

        // instantiate the hole by forcing a deallocation on the new memory
        self.deallocate(NonNull::new_unchecked(top as *mut u8), layout);

        // save extra bytes given to extend that weren't aligned to the hole size
        self.pending_extend = (extend_by - new_hole_size) as u8;
    }
}

unsafe fn make_hole(addr: *mut u8, size: usize) -> NonNull<Hole> {
    let hole_addr = addr.cast::<Hole>();
    debug_assert_eq!(
        addr as usize % align_of::<Hole>(),
        0,
        "Hole address not aligned!",
    );
    // Fresh hole with `next == None` (encoded `0` at its own slot).
    hole_addr.write(Hole { size, next_enc: 0 });
    NonNull::new_unchecked(hole_addr)
}

impl Cursor {
    fn try_insert_back(self, node: NonNull<Hole>, bottom: *mut u8) -> Result<Self, Self> {
        // Covers the case where the new hole exists BEFORE the current pointer,
        // which only happens when previous is the stub pointer
        if node < self.hole {
            let node_u8 = node.as_ptr().cast::<u8>();
            let node_size = unsafe { node.as_ref().size };
            let hole_u8 = self.hole.as_ptr().cast::<u8>();

            assert!(
                node_u8.wrapping_add(node_size) <= hole_u8,
                "Freed node aliases existing hole! Bad free?",
            );
            debug_assert_eq!(self.previous().size, 0);

            let Cursor {
                mut prev,
                hole,
                top,
                bottom: cbottom,
                prev_is_head,
            } = self;
            unsafe {
                let mut node = check_merge_bottom(node, bottom);
                // `prev` is the dummy head here (previous().size == 0), so encode
                // its link with the head slot; `node` is a real hole.
                prev.as_mut().store_next(prev_is_head, Some(node));
                node.as_mut().store_next(false, Some(hole));
            }
            Ok(Cursor {
                prev,
                hole: node,
                top,
                bottom: cbottom,
                prev_is_head,
            })
        } else {
            Err(self)
        }
    }

    fn try_insert_after(&mut self, mut node: NonNull<Hole>) -> Result<(), ()> {
        let bottom = self.bottom as usize;
        let top = self.top as usize;
        let node_u8 = node.as_ptr().cast::<u8>();
        let node_size = unsafe { node.as_ref().size };

        // `self.hole` is always a real hole in a live cursor (never the head).
        let maybe_next = unsafe { self.hole.as_ref() }.next_checked(false, bottom, top);

        // If we have a next, does the node overlap next?
        if let Some(next) = maybe_next {
            if node < next {
                let node_u8 = node_u8 as *const u8;
                assert!(
                    node_u8.wrapping_add(node_size) <= next.as_ptr().cast::<u8>(),
                    "Freed node aliases existing hole! Bad free?",
                );
            } else {
                // The new hole isn't between current and next.
                return Err(());
            }
        }

        // At this point, we either have no "next" pointer, or the hole is
        // between current and "next". The following assert can only trigger
        // if we've gotten our list out of order.
        debug_assert!(self.hole < node, "Hole list out of order?");

        let hole_u8 = self.hole.as_ptr().cast::<u8>();
        let hole_size = self.current().size;

        // Does hole overlap node?
        assert!(
            hole_u8.wrapping_add(hole_size) <= node_u8,
            "Freed node ({:?}) aliases existing hole ({:?}[{}])! Bad free?",
            node_u8,
            hole_u8,
            hole_size,
        );

        // All good! Let's insert that after. `hole.next = node; node.next = old`.
        unsafe {
            self.hole.as_mut().store_next(false, Some(node));
            node.as_mut().store_next(false, maybe_next);
        }

        Ok(())
    }

    // Merge the current node with up to n following nodes
    fn try_merge_next_n(self, max: usize) {
        let Cursor {
            prev: _,
            mut hole,
            top,
            bottom,
            ..
        } = self;
        let bottom_u = bottom as usize;
        let top_u = top as usize;

        for _ in 0..max {
            // Is there a next node? (`hole` is always a real hole.)
            let next = match unsafe { hole.as_ref() }.next_checked(false, bottom_u, top_u) {
                Some(next) => next,
                None => {
                    // Since there is no NEXT node, we need to check whether the current
                    // hole SHOULD extend to the end, but doesn't. This would happen when
                    // there isn't enough remaining space to place a hole after the current
                    // node's placement.
                    check_merge_top(hole, top);
                    return;
                }
            };

            // Can we directly merge these? e.g. are they touching?
            //
            // NOTE: Because we always use `HoleList::align_layout`, the size of
            // the new hole is always "rounded up" to cover any partial gaps that
            // would have occurred. For this reason, we DON'T need to "round up"
            // to account for an unaligned hole spot.
            let hole_u8 = hole.as_ptr().cast::<u8>();
            let hole_sz = unsafe { hole.as_ref().size };
            let next_u8 = next.as_ptr().cast::<u8>();
            let end = hole_u8.wrapping_add(hole_sz);

            let touching = end == next_u8;

            if touching {
                let next_sz;
                let next_next;
                unsafe {
                    let next_ref = next.as_ref();
                    next_sz = next_ref.size;
                    // `next` is absorbed into `hole`; carry its (validated) link forward.
                    next_next = next_ref.next_checked(false, bottom_u, top_u);
                }
                unsafe {
                    let hole_mut = hole.as_mut();
                    hole_mut.store_next(false, next_next);
                    hole_mut.size += next_sz;
                }
                // Okay, we just merged the next item. DON'T move the cursor, as we can
                // just try to merge the next_next, which is now our next.
            } else {
                // Welp, not touching, can't merge. Move to the next node.
                hole = next;
            }
        }
    }
}

/// Frees the allocation given by `(addr, size)`. It starts at the given hole and walks the list to
/// find the correct place (the list is sorted by address).
fn deallocate(list: &mut HoleList, addr: *mut u8, size: usize) {
    // Start off by just making this allocation a hole where it stands.
    // We'll attempt to merge it with other nodes once we figure out where
    // it should live
    let hole = unsafe { make_hole(addr, size) };

    // Now, try to get a cursor to the list - this only works if we have at least
    // one non-"dummy" hole in the list
    let cursor = if let Some(cursor) = list.cursor() {
        cursor
    } else {
        // Oh hey, there are no "real" holes at all. That means this just
        // becomes the only "real" hole! Check if this is touching the end
        // or the beginning of the allocation range
        let hole = check_merge_bottom(hole, list.bottom);
        check_merge_top(hole, list.top);
        // `first` is the dummy head — encode with the head slot.
        list.first.store_next(true, Some(hole));
        return;
    };

    // First, check if we can just insert this node at the top of the list. If the
    // insertion succeeded, then our cursor now points to the NEW node, behind the
    // previous location the cursor was pointing to.
    //
    // Otherwise, our cursor will point at the current non-"dummy" head of the list
    let (cursor, n) = match cursor.try_insert_back(hole, list.bottom) {
        Ok(cursor) => {
            // Yup! It lives at the front of the list. Hooray! Attempt to merge
            // it with just ONE next node, since it is at the front of the list
            (cursor, 1)
        }
        Err(mut cursor) => {
            // Nope. It lives somewhere else. Advance the list until we find its home
            while let Err(()) = cursor.try_insert_after(hole) {
                cursor = cursor
                    .next()
                    .expect("Reached end of holes without finding deallocation hole!");
            }
            // Great! We found a home for it, our cursor is now JUST BEFORE the new
            // node we inserted, so we need to try to merge up to twice: One to combine
            // the current node to the new node, then once more to combine the new node
            // with the node after that.
            (cursor, 2)
        }
    };

    // We now need to merge up to two times to combine the current node with the next
    // two nodes.
    cursor.try_merge_next_n(n);
}

#[cfg(test)]
pub mod test {
    use super::HoleList;
    use crate::{align_down_size, test::new_heap};
    use core::mem::size_of;
    use std::{alloc::Layout, convert::TryInto, prelude::v1::*, ptr::NonNull};

    #[test]
    fn cursor() {
        let mut heap = new_heap();
        let curs = heap.holes.cursor().unwrap();
        // This is the "dummy" node
        assert_eq!(curs.previous().size, 0);
        // This is the "full" heap
        assert_eq!(
            curs.current().size,
            align_down_size(1000, size_of::<usize>())
        );
        // There is no other hole
        assert!(curs.next().is_none());
    }

    #[test]
    fn aff() {
        let mut heap = new_heap();
        let reqd = Layout::from_size_align(256, 1).unwrap();
        let _ = heap.allocate_first_fit(reqd).unwrap();
    }

    /// Tests `HoleList::new` with the minimal allowed `hole_size`.
    #[test]
    fn hole_list_new_min_size() {
        // define an array of `u64` instead of `u8` for alignment
        static mut HEAP: [u64; 2] = [0; 2];
        let heap_start = unsafe { HEAP.as_ptr() as usize };
        let heap =
            unsafe { HoleList::new(HEAP.as_mut_ptr().cast(), 2 * core::mem::size_of::<usize>()) };
        assert_eq!(heap.bottom as usize, heap_start);
        assert_eq!(heap.top as usize, heap_start + 2 * size_of::<usize>());
        assert_eq!(heap.first.size, 0); // dummy
        // Head link decodes (via the move-invariant head slot) to the single hole.
        assert_eq!(heap.head_next(), Some(NonNull::new(heap.bottom.cast()).unwrap()));
        assert_eq!(
            unsafe { heap.head_next().unwrap().as_ref() }.size,
            2 * core::mem::size_of::<usize>()
        );
        assert_eq!(
            unsafe { heap.head_next().unwrap().as_ref() }.decoded_next(false),
            None
        );
    }

    /// Tests that `HoleList::new` aligns the `hole_addr` correctly and adjusts the size
    /// accordingly.
    #[test]
    fn hole_list_new_align() {
        // define an array of `u64` instead of `u8` for alignment
        static mut HEAP: [u64; 3] = [0; 3];

        let heap_start: *mut u8 = unsafe { HEAP.as_mut_ptr().add(1) }.cast();
        // initialize the HoleList with a hole_addr one byte before `heap_start`
        // -> the function should align it up to `heap_start`
        let heap =
            unsafe { HoleList::new(heap_start.sub(1), 2 * core::mem::size_of::<usize>() + 1) };
        assert_eq!(heap.bottom, heap_start);
        assert_eq!(heap.top.cast(), unsafe {
            // one byte less than the `hole_size` given to `new` because of alignment
            heap_start.add(2 * core::mem::size_of::<usize>())
        });

        assert_eq!(heap.first.size, 0); // dummy
        assert_eq!(heap.head_next(), Some(NonNull::new(heap.bottom.cast()).unwrap()));
        assert_eq!(
            unsafe { heap.head_next().unwrap().as_ref() }.size,
            unsafe { heap.top.offset_from(heap.bottom) }
                .try_into()
                .unwrap()
        );
        assert_eq!(
            unsafe { heap.head_next().unwrap().as_ref() }.decoded_next(false),
            None
        );
    }

    #[test]
    #[should_panic]
    fn hole_list_new_too_small() {
        // define an array of `u64` instead of `u8` for alignment
        static mut HEAP: [u64; 3] = [0; 3];

        let heap_start: *mut u8 = unsafe { HEAP.as_mut_ptr().add(1) }.cast();
        // initialize the HoleList with a hole_addr one byte before `heap_start`
        // -> the function should align it up to `heap_start`, but then the
        // available size is too small to store a hole -> it should panic
        unsafe { HoleList::new(heap_start.sub(1), 2 * core::mem::size_of::<usize>()) };
    }

    #[test]
    #[should_panic]
    fn extend_empty() {
        unsafe { HoleList::empty().extend(16) };
    }
}

// ── Freelist-hardening host KAT (the cheapest proof layer) ────────────────
//
// Race-free: uses the explicit-cookie `*_with` helpers so parallel test
// threads never fight over the global `FREELIST_COOKIE`. Proves the four
// properties the spec requires of the encode/validate predicate.
#[cfg(test)]
mod guard_tests {
    use super::{
        decode_with, encode_with, validate_link, validate_with, HeapCorruption, Hole,
    };
    use core::mem::align_of;
    use std::prelude::v1::*;

    // Arbitrary but hole-slot-shaped addresses (offset +8 = the `next` field).
    fn slot_a() -> *const usize {
        0x0000_7f00_0001_0008usize as *const usize
    }
    fn slot_b() -> *const usize {
        0x0000_7f00_0002_0008usize as *const usize
    }

    const COOKIES: &[u64] = &[
        0,
        1,
        0xDEAD_BEEF_CAFE_F00D,
        0x0F1E_2D3C_4B5A_6978,
        0xFFFF_FFFF_FFFF_FFFF,
        0xA5A5_5A5A_C3C3_3C3C,
    ];

    /// (a) `decode(encode(p, slot), slot) == p` round-trips for many p / slot / cookie.
    #[test]
    fn roundtrip() {
        let ptrs = [
            0usize,
            8,
            16,
            0x1000,
            0x10_0000,
            0xFFFF_9999_0000_0000,
            usize::MAX & !0b111,
        ];
        for &cookie in COOKIES {
            for &slot in &[slot_a(), slot_b()] {
                for &p in &ptrs {
                    assert_eq!(
                        decode_with(encode_with(p, slot, cookie), slot, cookie),
                        p,
                        "roundtrip failed p={:#x} cookie={:#x}",
                        p,
                        cookie
                    );
                }
            }
        }
    }

    /// (d) `p == 0` (stored `0`) decodes to the list tail `Ok(None)`.
    #[test]
    fn zero_is_tail() {
        let bottom = 0x10_000usize;
        let top = bottom + 16;
        for &cookie in COOKIES {
            // A stored 0 is always the tail, regardless of slot/cookie.
            assert_eq!(validate_with(0, slot_a(), cookie, bottom, top), Ok(None));
            // And encoding None yields exactly 0.
            assert_eq!(encode_with(0, slot_a(), cookie), 0);
        }
        // The live predicate (global cookie) agrees.
        assert_eq!(validate_link(0, slot_a(), bottom, top), Ok(None));
    }

    /// (b) Flipping ANY single bit of a valid encoded word makes validation fail
    /// (the only exception being a flip that zeroes the decoded pointer — that
    /// reads as the list tail `None`, a leak not a wild follow, and is explicitly
    /// out of scope). Uses a realistic multi-bit heap base so the pathological
    /// "pointer is a single power-of-two bit" case never arises.
    #[test]
    fn single_bit_flip_rejected() {
        // Real kernel-heap-shaped base (many set bits), span = one hole slot.
        let bottom = 0xFFFF_9999_0000_0008usize;
        let top = bottom + 16; // only `bottom` is a valid aligned in-range hole
        let p = bottom;
        assert_eq!(p % align_of::<Hole>(), 0);
        for &cookie in COOKIES {
            for &slot in &[slot_a(), slot_b()] {
                let enc = encode_with(p, slot, cookie);
                // Pristine encoding validates to the original pointer.
                assert_eq!(validate_with(enc, slot, cookie, bottom, top), Ok(Some(p)));
                for bit in 0..usize::BITS {
                    let corrupt = enc ^ (1usize << bit);
                    if decode_with(corrupt, slot, cookie) == 0 {
                        // Decodes to the tail (None) — accepted leak, not a follow.
                        continue;
                    }
                    assert!(
                        validate_with(corrupt, slot, cookie, bottom, top).is_err(),
                        "bit {} flip not rejected (cookie={:#x} enc={:#x})",
                        bit,
                        cookie,
                        enc
                    );
                }
            }
        }
    }

    /// (c) An encoding built for slot_a, fed at slot_b, decodes wrong and fails.
    #[test]
    fn wrong_slot_rejected() {
        let bottom = 0x10_000usize;
        let top = bottom + 16;
        let p = bottom;
        for &cookie in COOKIES {
            let enc = encode_with(p, slot_a(), cookie);
            assert_ne!(
                decode_with(enc, slot_b(), cookie),
                p,
                "location tie absent for cookie={:#x}",
                cookie
            );
            assert!(validate_with(enc, slot_b(), cookie, bottom, top).is_err());
        }
    }

    /// The live-path predicate itself rejects misaligned + out-of-range decodes
    /// (exercised at the default global cookie, which parallel tests never mutate).
    #[test]
    fn live_predicate_rejects() {
        let bottom = 0x10_000usize;
        let top = bottom + 16;
        let slot = slot_a();
        // Misaligned: encode an odd address.
        let bad = super::encode(bottom + 1, slot);
        assert_eq!(
            validate_link(bad, slot, bottom, top),
            Err(HeapCorruption::Misaligned)
        );
        // Out of range: aligned but far outside the span.
        let far = super::encode(bottom + 0x1000, slot);
        assert_eq!(
            validate_link(far, slot, bottom, top),
            Err(HeapCorruption::OutOfRange)
        );
    }
}
