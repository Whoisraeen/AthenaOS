//! Misc Linux `EXPORT_SYMBOL` utility helpers (`lib/*.c`): math, CRC, and the
//! generic `sort`. Pure functions over raw pointers / scalars — no allocation.

use core::ffi::c_void;

// ── DRM device-state + misc helpers (M4 gauge bucket-B) ──────────────────────
// The daemon's device is always present/powered, exposes no sysfs, and never
// reclaims — these report that truthfully.

/// `drm_dev_enter(dev, idx)` — begins a "device alive" section. The daemon device
/// never unplugs, so always succeed (and set the cookie to 0).
#[no_mangle]
pub unsafe extern "C" fn drm_dev_enter(_dev: *mut c_void, idx: *mut i32) -> bool {
    if !idx.is_null() {
        *idx = 0;
    }
    true
}
#[no_mangle]
pub extern "C" fn drm_dev_exit(_idx: i32) {}
#[no_mangle]
pub extern "C" fn drm_need_swiotlb(_dma_bits: i32) -> bool {
    false
}

/// sysfs attributes — the daemon exposes none, so creates succeed (no-op) and
/// reads emit empty.
#[no_mangle]
pub extern "C" fn device_create_file(_dev: *mut c_void, _attr: *const c_void) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn device_remove_file(_dev: *mut c_void, _attr: *const c_void) {}
#[no_mangle]
pub unsafe extern "C" fn sysfs_emit(buf: *mut u8, _fmt: *const u8) -> i32 {
    if !buf.is_null() {
        *buf = 0;
    }
    0
}
#[no_mangle]
pub unsafe extern "C" fn sysfs_emit_at(buf: *mut u8, at: i32, _fmt: *const u8) -> i32 {
    if !buf.is_null() && at >= 0 {
        *buf.add(at as usize) = 0;
    }
    0
}

/// `memalloc_noreclaim_save/restore` — the "no reclaim during this alloc" scope.
/// The daemon heap never reclaims, so the flag is a no-op token.
#[no_mangle]
pub extern "C" fn memalloc_noreclaim_save() -> u32 {
    0
}
#[no_mangle]
pub extern "C" fn memalloc_noreclaim_restore(_flags: u32) {}

#[no_mangle]
pub extern "C" fn pm_resume_via_firmware() -> bool {
    false
}
#[no_mangle]
pub extern "C" fn rwsem_is_contended(_sem: *mut c_void) -> bool {
    false
}
/// `printk_ratelimit()` — always allow (the daemon's log is not rate-limited).
#[no_mangle]
pub extern "C" fn printk_ratelimit() -> i32 {
    1
}

// ── byteorder ── x86_64 is little-endian, so cpu<->le is identity; cpu<->be swaps.
// Usually macros in Linux, but the shim externs them, so provide real symbols.
#[no_mangle]
pub extern "C" fn cpu_to_le16(x: u16) -> u16 {
    x
}
#[no_mangle]
pub extern "C" fn cpu_to_le32(x: u32) -> u32 {
    x
}
#[no_mangle]
pub extern "C" fn cpu_to_le64(x: u64) -> u64 {
    x
}
#[no_mangle]
pub extern "C" fn le16_to_cpu(x: u16) -> u16 {
    x
}
#[no_mangle]
pub extern "C" fn le32_to_cpu(x: u32) -> u32 {
    x
}
#[no_mangle]
pub extern "C" fn le64_to_cpu(x: u64) -> u64 {
    x
}
#[no_mangle]
pub extern "C" fn cpu_to_be16(x: u16) -> u16 {
    x.swap_bytes()
}
#[no_mangle]
pub extern "C" fn cpu_to_be32(x: u32) -> u32 {
    x.swap_bytes()
}
#[no_mangle]
pub extern "C" fn be16_to_cpu(x: u16) -> u16 {
    x.swap_bytes()
}
#[no_mangle]
pub extern "C" fn be32_to_cpu(x: u32) -> u32 {
    x.swap_bytes()
}

/// `is_power_of_2(n)` — true iff n is a power of two.
#[no_mangle]
pub extern "C" fn is_power_of_2(n: u64) -> bool {
    n != 0 && (n & (n - 1)) == 0
}

/// `memset32(s, v, count)` — fill `count` u32 words with `v`; returns `s`.
#[no_mangle]
pub unsafe extern "C" fn memset32(s: *mut u32, v: u32, count: usize) -> *mut u32 {
    for i in 0..count {
        *s.add(i) = v;
    }
    s
}
/// `memset64(s, v, count)` — fill `count` u64 words with `v`; returns `s`.
#[no_mangle]
pub unsafe extern "C" fn memset64(s: *mut u64, v: u64, count: usize) -> *mut u64 {
    for i in 0..count {
        *s.add(i) = v;
    }
    s
}

/// `gcd(a, b)` — greatest common divisor (Euclid). Linux `lib/gcd.c`.
#[no_mangle]
pub extern "C" fn gcd(a: u64, b: u64) -> u64 {
    let (mut a, mut b) = (a, b);
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

/// `crc16(crc, buffer, len)` — CRC-16-ANSI/IBM (reflected poly 0xA001), the
/// bit-at-a-time form of Linux `lib/crc16.c` (table-free, identical output).
#[no_mangle]
pub unsafe extern "C" fn crc16(mut crc: u16, buffer: *const u8, len: usize) -> u16 {
    for i in 0..len {
        crc ^= *buffer.add(i) as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xA001
            } else {
                crc >> 1
            };
        }
    }
    crc
}

#[inline]
fn is_ws(c: u8) -> bool {
    c == b' ' || c == b'\t' || c == b'\n' || c == b'\r'
}

/// `sscanf(str, fmt, ...)` — parse `str` per `fmt`, storing results into the
/// vararg output pointers; returns the number of fields successfully matched (or
/// EOF semantics: stops at first mismatch). Supports whitespace, literals, `%%`,
/// and `%[*][width][l|ll|h|hh|z]{d,i,u,x,X,o,s,c}` — the surface kernel drivers use.
#[no_mangle]
pub unsafe extern "C" fn sscanf(s: *const u8, fmt: *const u8, mut ap: ...) -> i32 {
    let mut si: isize = 0;
    let mut fi: isize = 0;
    let mut count: i32 = 0;
    let sch = |i: isize| -> u8 { *s.offset(i) };
    let fch = |i: isize| -> u8 { *fmt.offset(i) };

    loop {
        let f = fch(fi);
        if f == 0 {
            break;
        }
        if is_ws(f) {
            fi += 1;
            while is_ws(sch(si)) {
                si += 1;
            }
            continue;
        }
        if f != b'%' {
            if sch(si) != f {
                break;
            }
            si += 1;
            fi += 1;
            continue;
        }
        // conversion
        fi += 1;
        let mut suppress = false;
        if fch(fi) == b'*' {
            suppress = true;
            fi += 1;
        }
        let mut width: usize = 0;
        let mut have_width = false;
        while fch(fi).is_ascii_digit() {
            have_width = true;
            width = width * 10 + (fch(fi) - b'0') as usize;
            fi += 1;
        }
        // length: longness 0=int 1=long 2=long-long/size  -1=short  -2=char
        let mut longness: i32 = 0;
        loop {
            match fch(fi) {
                b'l' => {
                    longness += 1;
                    fi += 1;
                }
                b'h' => {
                    longness -= 1;
                    fi += 1;
                }
                b'z' | b'j' | b't' => {
                    longness = 2;
                    fi += 1;
                }
                _ => break,
            }
        }
        let spec = fch(fi);
        fi += 1;
        if spec != b'c' && spec != b'%' {
            while is_ws(sch(si)) {
                si += 1;
            }
        }
        match spec {
            b'%' => {
                if sch(si) != b'%' {
                    break;
                }
                si += 1;
            }
            b'd' | b'i' | b'u' | b'x' | b'X' | b'o' => {
                let base: u64 = match spec {
                    b'x' | b'X' => 16,
                    b'o' => 8,
                    _ => 10,
                };
                let signed = spec == b'd' || spec == b'i';
                let start = si;
                let mut neg = false;
                if signed && (sch(si) == b'-' || sch(si) == b'+') {
                    neg = sch(si) == b'-';
                    si += 1;
                }
                let mut val: u64 = 0;
                let mut digits = 0;
                loop {
                    if have_width && (si - start) as usize >= width {
                        break;
                    }
                    let c = sch(si);
                    let d = match c {
                        b'0'..=b'9' => (c - b'0') as u64,
                        b'a'..=b'f' if base == 16 => (c - b'a' + 10) as u64,
                        b'A'..=b'F' if base == 16 => (c - b'A' + 10) as u64,
                        _ => break,
                    };
                    if d >= base {
                        break;
                    }
                    val = val * base + d;
                    si += 1;
                    digits += 1;
                }
                if digits == 0 {
                    break;
                }
                if !suppress {
                    let stored = if neg {
                        (val as i64).wrapping_neg() as u64
                    } else {
                        val
                    };
                    if longness >= 1 {
                        let p = ap.next_arg::<*mut u64>();
                        if !p.is_null() {
                            *p = stored;
                        }
                    } else if longness <= -2 {
                        let p = ap.next_arg::<*mut u8>();
                        if !p.is_null() {
                            *p = stored as u8;
                        }
                    } else if longness == -1 {
                        let p = ap.next_arg::<*mut u16>();
                        if !p.is_null() {
                            *p = stored as u16;
                        }
                    } else {
                        let p = ap.next_arg::<*mut u32>();
                        if !p.is_null() {
                            *p = stored as u32;
                        }
                    }
                    count += 1;
                }
            }
            b's' => {
                let dst = if suppress {
                    core::ptr::null_mut()
                } else {
                    ap.next_arg::<*mut u8>()
                };
                let start = si;
                let mut n = 0;
                loop {
                    if have_width && (si - start) as usize >= width {
                        break;
                    }
                    let c = sch(si);
                    if c == 0 || is_ws(c) {
                        break;
                    }
                    if !dst.is_null() {
                        *dst.add(n) = c;
                    }
                    si += 1;
                    n += 1;
                }
                if n == 0 {
                    break;
                }
                if !dst.is_null() {
                    *dst.add(n) = 0;
                }
                if !suppress {
                    count += 1;
                }
            }
            b'c' => {
                let w = if have_width { width } else { 1 };
                let dst = if suppress {
                    core::ptr::null_mut()
                } else {
                    ap.next_arg::<*mut u8>()
                };
                let mut got = 0;
                for k in 0..w {
                    let c = sch(si);
                    if c == 0 {
                        break;
                    }
                    if !dst.is_null() {
                        *dst.add(k) = c;
                    }
                    si += 1;
                    got += 1;
                }
                if got == 0 {
                    break;
                }
                if !suppress {
                    count += 1;
                }
            }
            _ => break,
        }
    }
    count
}

type CmpFunc = unsafe extern "C" fn(*const c_void, *const c_void) -> i32;
type SwapFunc = unsafe extern "C" fn(*mut c_void, *mut c_void, i32);

#[inline]
unsafe fn elem(base: *mut u8, i: usize, size: usize) -> *mut c_void {
    base.add(i * size) as *mut c_void
}

#[inline]
unsafe fn do_swap(a: *mut c_void, b: *mut c_void, size: usize, swap: Option<SwapFunc>) {
    match swap {
        Some(s) => s(a, b, size as i32),
        None => {
            // Default byte-wise swap (Linux falls back to this when swap == NULL).
            let pa = a as *mut u8;
            let pb = b as *mut u8;
            for k in 0..size {
                core::ptr::swap(pa.add(k), pb.add(k));
            }
        }
    }
}

/// `sort(base, num, size, cmp, swap)` — Linux's generic in-place heapsort
/// (`lib/sort.c`). `cmp(a,b) < 0` means a precedes b; `swap` may be NULL (then a
/// byte-wise swap is used). Ascending order.
#[no_mangle]
pub unsafe extern "C" fn sort(
    base: *mut c_void,
    num: usize,
    size: usize,
    cmp: Option<CmpFunc>,
    swap: Option<SwapFunc>,
) {
    let Some(cmp) = cmp else { return };
    if base.is_null()
        || num <= 1
        || size == 0
        || size > i32::MAX as usize
        || num.checked_mul(size).is_none()
        || num * size > isize::MAX as usize
    {
        return;
    }
    let b = base as *mut u8;
    let mut start = num / 2;
    let mut end = num;
    while end > 1 {
        if start > 0 {
            start -= 1;
        } else {
            end -= 1;
            do_swap(elem(b, 0, size), elem(b, end, size), size, swap);
        }
        // sift a[start] down within [0, end)
        let mut root = start;
        loop {
            let Some(child) = root.checked_mul(2).and_then(|v| v.checked_add(1)) else {
                return;
            };
            if child >= end {
                break;
            }
            let mut c = child;
            if child + 1 < end && cmp(elem(b, child, size), elem(b, child + 1, size)) < 0 {
                c = child + 1;
            }
            if cmp(elem(b, root, size), elem(b, c, size)) < 0 {
                do_swap(elem(b, root, size), elem(b, c, size), size, swap);
                root = c;
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod sort_tests {
    use super::*;

    unsafe extern "C" fn compare_u32(a: *const c_void, b: *const c_void) -> i32 {
        let a = *a.cast::<u32>();
        let b = *b.cast::<u32>();
        match a.cmp(&b) {
            core::cmp::Ordering::Less => -1,
            core::cmp::Ordering::Equal => 0,
            core::cmp::Ordering::Greater => 1,
        }
    }

    #[test]
    fn sort_orders_duplicates_and_extremes() {
        let mut values = [9u32, 0, u32::MAX, 4, 4, 2, 17, 1];
        unsafe {
            sort(
                values.as_mut_ptr().cast(),
                values.len(),
                core::mem::size_of::<u32>(),
                Some(compare_u32),
                None,
            );
        }
        assert_eq!(values, [0, 1, 2, 4, 4, 9, 17, u32::MAX]);
    }
}
