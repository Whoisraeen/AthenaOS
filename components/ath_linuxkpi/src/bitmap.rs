//! `find_bit` / `bitmap` helpers — scan and mutate arrays of bits.
//!
//! Drivers manage dense resources with bitmaps: MSI-X vector pools, GPU VMID/
//! doorbell allocation, ring-slot occupancy. The user-facing `find_first_bit`/
//! `for_each_set_bit`/`bitmap_*` are header inlines that bottom out on the
//! exported `_find_*` and `__bitmap_*` symbols, which carry the real word-walk
//! and partial-last-word masking. A stub that ignored the size bound would scan
//! into uninitialized memory and hand back a bogus index.
//!
//! Bits are packed LSB-first into an array of `unsigned long` (64-bit on LP64),
//! exactly as Linux lays them out, so a real `.ko` and the host KAT agree.

const BITS_PER_LONG: u64 = 64;

/// Core scanner: lowest index in `[start, size)` whose bit is set (or clear, if
/// `find_zero`). Returns `size` when none is found. Bits at/after `size` are
/// treated as absent (Linux's contract), guarded by the `< size` check on the
/// lowest candidate in each word.
#[inline]
unsafe fn find_bit(addr: *const u64, size: u64, start: u64, find_zero: bool) -> u64 {
    if start >= size {
        return size;
    }
    let mut idx = start;
    while idx < size {
        let raw = *addr.add((idx / BITS_PER_LONG) as usize);
        let w = if find_zero { !raw } else { raw };
        let bit_in_word = idx % BITS_PER_LONG;
        let masked = w & (!0u64 << bit_in_word);
        if masked != 0 {
            let found = (idx / BITS_PER_LONG) * BITS_PER_LONG + masked.trailing_zeros() as u64;
            return if found < size { found } else { size };
        }
        idx = (idx / BITS_PER_LONG + 1) * BITS_PER_LONG;
    }
    size
}

/// `_find_first_bit(addr, size)`.
#[no_mangle]
pub unsafe extern "C" fn _find_first_bit(addr: *const u64, size: u64) -> u64 {
    find_bit(addr, size, 0, false)
}

/// `_find_next_bit(addr, nbits, start)`.
#[no_mangle]
pub unsafe extern "C" fn _find_next_bit(addr: *const u64, nbits: u64, start: u64) -> u64 {
    find_bit(addr, nbits, start, false)
}

/// `_find_first_zero_bit(addr, size)`.
#[no_mangle]
pub unsafe extern "C" fn _find_first_zero_bit(addr: *const u64, size: u64) -> u64 {
    find_bit(addr, size, 0, true)
}

/// `_find_next_zero_bit(addr, size, offset)`.
#[no_mangle]
pub unsafe extern "C" fn _find_next_zero_bit(addr: *const u64, size: u64, offset: u64) -> u64 {
    find_bit(addr, size, offset, true)
}

/// `__bitmap_set(map, start, len)`.
#[no_mangle]
pub unsafe extern "C" fn __bitmap_set(map: *mut u64, start: u32, len: i32) {
    if map.is_null() || len <= 0 {
        return;
    }
    for i in start..start + (len as u32) {
        let w = map.add((i / 64) as usize);
        *w |= 1u64 << (i % 64);
    }
}

/// `__bitmap_clear(map, start, len)`.
#[no_mangle]
pub unsafe extern "C" fn __bitmap_clear(map: *mut u64, start: u32, len: i32) {
    if map.is_null() || len <= 0 {
        return;
    }
    for i in start..start + (len as u32) {
        let w = map.add((i / 64) as usize);
        *w &= !(1u64 << (i % 64));
    }
}

/// Mask of the valid bits in the final (possibly partial) word for `nbits`.
#[inline]
fn last_word_mask(nbits: u64) -> u64 {
    let rem = nbits % BITS_PER_LONG;
    if rem == 0 {
        !0u64
    } else {
        (1u64 << rem) - 1
    }
}

/// `__bitmap_weight(bitmap, nbits)` — count set bits in the first `nbits`.
#[no_mangle]
pub unsafe extern "C" fn __bitmap_weight(bitmap: *const u64, nbits: u32) -> i32 {
    if bitmap.is_null() || nbits == 0 {
        return 0;
    }
    let nbits = nbits as u64;
    let full = (nbits / BITS_PER_LONG) as usize;
    let mut count = 0u32;
    for i in 0..full {
        count += (*bitmap.add(i)).count_ones();
    }
    let rem = nbits % BITS_PER_LONG;
    if rem != 0 {
        count += (*bitmap.add(full) & last_word_mask(nbits)).count_ones();
    }
    count as i32
}

/// `__bitmap_empty(bitmap, nbits)` → 1 if no bit in the first `nbits` is set.
#[no_mangle]
pub unsafe extern "C" fn __bitmap_empty(bitmap: *const u64, nbits: u32) -> i32 {
    (__bitmap_weight(bitmap, nbits) == 0) as i32
}

/// `__bitmap_full(bitmap, nbits)` → 1 if every bit in the first `nbits` is set.
#[no_mangle]
pub unsafe extern "C" fn __bitmap_full(bitmap: *const u64, nbits: u32) -> i32 {
    (__bitmap_weight(bitmap, nbits) == nbits as i32) as i32
}

/// Number of `unsigned long` words covering `nbits` (Linux `BITS_TO_LONGS`).
#[inline]
fn words_of(nbits: u32) -> usize {
    (nbits as u64).div_ceil(BITS_PER_LONG) as usize
}

/// `__bitmap_or(dst, src1, src2, nbits)` — `dst = src1 | src2`.
#[no_mangle]
pub unsafe extern "C" fn __bitmap_or(
    dst: *mut u64,
    src1: *const u64,
    src2: *const u64,
    nbits: u32,
) {
    for i in 0..words_of(nbits) {
        *dst.add(i) = *src1.add(i) | *src2.add(i);
    }
}

/// `__bitmap_and(dst, src1, src2, nbits)` → nonzero if any result bit is set.
#[no_mangle]
pub unsafe extern "C" fn __bitmap_and(
    dst: *mut u64,
    src1: *const u64,
    src2: *const u64,
    nbits: u32,
) -> i32 {
    let mut acc = 0u64;
    for i in 0..words_of(nbits) {
        let w = *src1.add(i) & *src2.add(i);
        *dst.add(i) = w;
        acc |= w;
    }
    (acc != 0) as i32
}

/// `__bitmap_complement(dst, src, nbits)` — `dst = ~src` over the covered words.
#[no_mangle]
pub unsafe extern "C" fn __bitmap_complement(dst: *mut u64, src: *const u64, nbits: u32) {
    for i in 0..words_of(nbits) {
        *dst.add(i) = !*src.add(i);
    }
}

/// `bitmap_zalloc(nbits, gfp)` — allocate a zeroed bitmap on the daemon heap.
#[no_mangle]
pub extern "C" fn bitmap_zalloc(nbits: u32, _gfp: u32) -> *mut u64 {
    crate::mm::kzalloc(words_of(nbits).max(1) * 8, 0) as *mut u64
}

/// `bitmap_free(bitmap)`.
#[no_mangle]
pub extern "C" fn bitmap_free(bitmap: *const u64) {
    if !bitmap.is_null() {
        crate::mm::kfree(bitmap as *mut u8);
    }
}

/// `bitmap_to_arr32(buf, bitmap, nbits)` — repack a `long`-array bitmap into a
/// `u32` array (each long splits into low/high halves).
#[no_mangle]
pub unsafe extern "C" fn bitmap_to_arr32(buf: *mut u32, bitmap: *const u64, nbits: u32) {
    let n32 = (nbits as u64).div_ceil(32) as usize;
    for i in 0..n32 {
        let w = *bitmap.add(i / 2);
        *buf.add(i) = if i % 2 == 0 {
            w as u32
        } else {
            (w >> 32) as u32
        };
    }
}

/// `bitmap_from_arr32` — pack a `u32[]` into the `unsigned long[]` (64-bit) bitmap;
/// inverse of [`bitmap_to_arr32`].
#[no_mangle]
pub unsafe extern "C" fn bitmap_from_arr32(bitmap: *mut u64, buf: *const u32, nbits: u32) {
    let n64 = (nbits as usize).div_ceil(64);
    for i in 0..n64 {
        *bitmap.add(i) = 0;
    }
    let n32 = (nbits as usize).div_ceil(32);
    for i in 0..n32 {
        let v = *buf.add(i) as u64;
        if i % 2 == 0 {
            *bitmap.add(i / 2) |= v;
        } else {
            *bitmap.add(i / 2) |= v << 32;
        }
    }
}

/// `__bitmap_intersects` — true if any bit is set in BOTH bitmaps within `nbits`
/// (the trailing partial word is masked, matching Linux).
#[no_mangle]
pub unsafe extern "C" fn __bitmap_intersects(a: *const u64, b: *const u64, nbits: u32) -> bool {
    let full = (nbits / 64) as usize;
    for i in 0..full {
        if *a.add(i) & *b.add(i) != 0 {
            return true;
        }
    }
    let rem = nbits % 64;
    if rem != 0 {
        let mask = (1u64 << rem) - 1;
        if *a.add(full) & *b.add(full) & mask != 0 {
            return true;
        }
    }
    false
}

/// `__sw_hweight32`/`64` — software population count (the out-of-line fallback the
/// kernel calls when the CPU lacks `POPCNT`; here just the native popcount).
#[no_mangle]
pub extern "C" fn __sw_hweight32(w: u32) -> u32 {
    w.count_ones()
}
#[no_mangle]
pub extern "C" fn __sw_hweight64(w: u64) -> u32 {
    w.count_ones()
}

/// libgcc/compiler-rt population-count intrinsics. gcc lowers `__builtin_popcount*`
/// (used by the kernel's inline `hweight*`) to these libcalls; Rust's
/// compiler_builtins does NOT provide them (rustc lowers `count_ones` inline), so
/// the freestanding C objects have no provider unless the shim supplies them.
#[no_mangle]
pub extern "C" fn __popcountsi2(a: u32) -> i32 {
    a.count_ones() as i32
}
#[no_mangle]
pub extern "C" fn __popcountdi2(a: u64) -> i32 {
    a.count_ones() as i32
}

// The non-underscore `find_*_bit` are inline wrappers in Linux; the shim externs
// them, so forward to the out-of-line `_find_*` impls above.
#[no_mangle]
pub unsafe extern "C" fn find_first_bit(addr: *const u64, size: u64) -> u64 {
    _find_first_bit(addr, size)
}
#[no_mangle]
pub unsafe extern "C" fn find_next_bit(addr: *const u64, size: u64, offset: u64) -> u64 {
    _find_next_bit(addr, size, offset)
}
#[no_mangle]
pub unsafe extern "C" fn find_first_zero_bit(addr: *const u64, size: u64) -> u64 {
    _find_first_zero_bit(addr, size)
}
#[no_mangle]
pub unsafe extern "C" fn find_next_zero_bit(addr: *const u64, size: u64, offset: u64) -> u64 {
    _find_next_zero_bit(addr, size, offset)
}

/// `bitmap_read(map, start, nbits)` — read an `nbits`-wide value starting at bit
/// `start`, possibly spanning a word boundary. Kernel contract: `nbits == 0` or
/// `nbits > BITS_PER_LONG` returns 0. Used by amdgpu_utils.h's capability
/// attributes; was an inert weak stub until the 2026-07-08 implicit-decl audit.
#[no_mangle]
pub unsafe extern "C" fn bitmap_read(map: *const u64, start: u64, nbits: u64) -> u64 {
    if nbits == 0 || nbits > 64 {
        return 0;
    }
    let index = (start / 64) as usize;
    let offset = start % 64;
    let space = 64 - offset;
    let mask = if nbits == 64 {
        u64::MAX
    } else {
        (1u64 << nbits) - 1
    };
    let lo = *map.add(index) >> offset;
    let value = if nbits <= space {
        lo
    } else {
        lo | (*map.add(index + 1) << space)
    };
    value & mask
}

/// `bitmap_write(map, value, start, nbits)` — write the low `nbits` of `value`
/// at bit `start`, possibly spanning a word boundary. `nbits == 0` or
/// `nbits > BITS_PER_LONG` is a no-op (kernel contract).
#[no_mangle]
pub unsafe extern "C" fn bitmap_write(map: *mut u64, value: u64, start: u64, nbits: u64) {
    if nbits == 0 || nbits > 64 {
        return;
    }
    let index = (start / 64) as usize;
    let offset = start % 64;
    let space = 64 - offset;
    let mask = if nbits == 64 {
        u64::MAX
    } else {
        (1u64 << nbits) - 1
    };
    let value = value & mask;
    let w = map.add(index);
    *w = (*w & !(mask << offset)) | (value << offset);
    if nbits > space {
        let hi = map.add(index + 1);
        let hi_mask = mask >> space;
        *hi = (*hi & !hi_mask) | (value >> space);
    }
}

#[cfg(test)]
mod rw_tests {
    use super::*;

    /// FAIL-able KAT: read-back across word boundaries, masking, and the
    /// degenerate widths — mirrors the kernel's lib/test_bitmap coverage.
    #[test]
    fn bitmap_read_write_round_trip() {
        let mut map = [0u64; 4];
        unsafe {
            // in-word
            bitmap_write(map.as_mut_ptr(), 0x2b, 3, 6);
            assert_eq!(bitmap_read(map.as_ptr(), 3, 6), 0x2b);
            // spanning the 64-bit boundary (start 60, width 8)
            bitmap_write(map.as_mut_ptr(), 0xa5, 60, 8);
            assert_eq!(bitmap_read(map.as_ptr(), 60, 8), 0xa5);
            // full-width at an unaligned offset
            bitmap_write(map.as_mut_ptr(), 0xdead_beef_cafe_f00d, 100, 64);
            assert_eq!(bitmap_read(map.as_ptr(), 100, 64), 0xdead_beef_cafe_f00d);
            // value wider than nbits is masked on write
            bitmap_write(map.as_mut_ptr(), 0xffff, 8, 4);
            assert_eq!(bitmap_read(map.as_ptr(), 8, 4), 0xf);
            // degenerate widths
            assert_eq!(bitmap_read(map.as_ptr(), 0, 0), 0);
            assert_eq!(bitmap_read(map.as_ptr(), 0, 65), 0);
            // neighbors survive: the 60..68 field still reads back
            assert_eq!(bitmap_read(map.as_ptr(), 60, 8), 0xa5);
        }
    }
}
