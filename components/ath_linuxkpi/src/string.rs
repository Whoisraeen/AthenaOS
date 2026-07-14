//! Linux `EXPORT_SYMBOL` string + memory primitives.
//!
//! These are real `mem*`/`str*` implementations the C compiler emits calls to
//! (memcpy/memset are implicit for struct copies and array init) and that Linux
//! drivers call explicitly. Every one must resolve or the `.ko` fails to link.
//! All operate on raw pointers — no allocation, no host round-trip.

use core::ffi::c_void;

#[no_mangle]
pub unsafe extern "C" fn memcpy(dest: *mut c_void, src: *const c_void, n: usize) -> *mut c_void {
    let d = dest as *mut u8;
    let s = src as *const u8;
    let mut i = 0;
    while i < n {
        *d.add(i) = *s.add(i);
        i += 1;
    }
    dest
}

#[no_mangle]
pub unsafe extern "C" fn memmove(dest: *mut c_void, src: *const c_void, n: usize) -> *mut c_void {
    let d = dest as *mut u8;
    let s = src as *const u8;
    if (d as usize) < (s as usize) {
        let mut i = 0;
        while i < n {
            *d.add(i) = *s.add(i);
            i += 1;
        }
    } else {
        let mut i = n;
        while i > 0 {
            i -= 1;
            *d.add(i) = *s.add(i);
        }
    }
    dest
}

#[no_mangle]
pub unsafe extern "C" fn memset(dest: *mut c_void, c: i32, n: usize) -> *mut c_void {
    let d = dest as *mut u8;
    let byte = c as u8;
    let mut i = 0;
    while i < n {
        *d.add(i) = byte;
        i += 1;
    }
    dest
}

#[no_mangle]
pub unsafe extern "C" fn memcmp(a: *const c_void, b: *const c_void, n: usize) -> i32 {
    let pa = a as *const u8;
    let pb = b as *const u8;
    let mut i = 0;
    while i < n {
        let x = *pa.add(i);
        let y = *pb.add(i);
        if x != y {
            return x as i32 - y as i32;
        }
        i += 1;
    }
    0
}

/// Linux `memchr`.
#[no_mangle]
pub unsafe extern "C" fn memchr(s: *const c_void, c: i32, n: usize) -> *mut c_void {
    let p = s as *const u8;
    let byte = c as u8;
    let mut i = 0;
    while i < n {
        if *p.add(i) == byte {
            return p.add(i) as *mut c_void;
        }
        i += 1;
    }
    core::ptr::null_mut()
}

#[no_mangle]
pub unsafe extern "C" fn strlen(s: *const u8) -> usize {
    if s.is_null() {
        return 0;
    }
    let mut n = 0;
    while *s.add(n) != 0 {
        n += 1;
    }
    n
}

#[no_mangle]
pub unsafe extern "C" fn strnlen(s: *const u8, maxlen: usize) -> usize {
    if s.is_null() {
        return 0;
    }
    let mut n = 0;
    while n < maxlen && *s.add(n) != 0 {
        n += 1;
    }
    n
}

#[no_mangle]
pub unsafe extern "C" fn strcmp(a: *const u8, b: *const u8) -> i32 {
    let mut i = 0;
    loop {
        let x = *a.add(i);
        let y = *b.add(i);
        if x != y {
            return x as i32 - y as i32;
        }
        if x == 0 {
            return 0;
        }
        i += 1;
    }
}

#[no_mangle]
pub unsafe extern "C" fn strncmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    let mut i = 0;
    while i < n {
        let x = *a.add(i);
        let y = *b.add(i);
        if x != y {
            return x as i32 - y as i32;
        }
        if x == 0 {
            return 0;
        }
        i += 1;
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn strcpy(dest: *mut u8, src: *const u8) -> *mut u8 {
    let mut i = 0;
    loop {
        let c = *src.add(i);
        *dest.add(i) = c;
        if c == 0 {
            break;
        }
        i += 1;
    }
    dest
}

#[no_mangle]
pub unsafe extern "C" fn strncpy(dest: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    let mut i = 0;
    while i < n {
        let c = *src.add(i);
        *dest.add(i) = c;
        if c == 0 {
            break;
        }
        i += 1;
    }
    // Linux strncpy zero-pads the remainder.
    while i < n {
        *dest.add(i) = 0;
        i += 1;
    }
    dest
}

#[no_mangle]
pub unsafe extern "C" fn strchr(s: *const u8, c: i32) -> *mut u8 {
    let target = c as u8;
    let mut i = 0;
    loop {
        let cur = *s.add(i);
        if cur == target {
            return s.add(i) as *mut u8;
        }
        if cur == 0 {
            return core::ptr::null_mut();
        }
        i += 1;
    }
}

#[no_mangle]
pub unsafe extern "C" fn strrchr(s: *const u8, c: i32) -> *mut u8 {
    let target = c as u8;
    let mut last: *mut u8 = core::ptr::null_mut();
    let mut i = 0;
    loop {
        let cur = *s.add(i);
        if cur == target {
            last = s.add(i) as *mut u8;
        }
        if cur == 0 {
            return last;
        }
        i += 1;
    }
}

/// Linux `strstr` — first occurrence of `needle` in `haystack`.
#[no_mangle]
pub unsafe extern "C" fn strstr(haystack: *const u8, needle: *const u8) -> *mut u8 {
    let nlen = strlen(needle);
    if nlen == 0 {
        return haystack as *mut u8;
    }
    let mut i = 0;
    loop {
        if *haystack.add(i) == 0 {
            return core::ptr::null_mut();
        }
        if strncmp(haystack.add(i), needle, nlen) == 0 {
            return haystack.add(i) as *mut u8;
        }
        i += 1;
    }
}

/// Linux `strnstr` — `strstr` bounded to the first `len` bytes of `haystack`.
#[no_mangle]
pub unsafe extern "C" fn strnstr(haystack: *const u8, needle: *const u8, len: usize) -> *mut u8 {
    let nlen = strlen(needle);
    if nlen == 0 {
        return haystack as *mut u8;
    }
    if nlen > len {
        return core::ptr::null_mut();
    }
    let mut i = 0;
    while i + nlen <= len {
        if *haystack.add(i) == 0 {
            return core::ptr::null_mut();
        }
        if strncmp(haystack.add(i), needle, nlen) == 0 {
            return haystack.add(i) as *mut u8;
        }
        i += 1;
    }
    core::ptr::null_mut()
}

#[inline]
fn ascii_lower(x: u8) -> u8 {
    if x.is_ascii_uppercase() {
        x + 32
    } else {
        x
    }
}

/// Linux `strncasecmp` — case-insensitive compare of the first `n` bytes.
#[no_mangle]
pub unsafe extern "C" fn strncasecmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    let mut i = 0;
    while i < n {
        let x = ascii_lower(*a.add(i));
        let y = ascii_lower(*b.add(i));
        if x != y {
            return x as i32 - y as i32;
        }
        if x == 0 {
            return 0;
        }
        i += 1;
    }
    0
}

/// Linux `strcasecmp` — case-insensitive compare (unbounded).
#[no_mangle]
pub unsafe extern "C" fn strcasecmp(a: *const u8, b: *const u8) -> i32 {
    let mut i = 0;
    loop {
        let x = ascii_lower(*a.add(i));
        let y = ascii_lower(*b.add(i));
        if x != y {
            return x as i32 - y as i32;
        }
        if x == 0 {
            return 0;
        }
        i += 1;
    }
}

/// Linux `strsep` — split `*s` at the first delimiter; advances `*s` past it,
/// NUL-terminates the token, returns the token (or NULL when `*s` is NULL).
#[no_mangle]
pub unsafe extern "C" fn strsep(s: *mut *mut u8, delim: *const u8) -> *mut u8 {
    let start = *s;
    if start.is_null() {
        return core::ptr::null_mut();
    }
    let mut i = 0;
    loop {
        let c = *start.add(i);
        if c == 0 {
            *s = core::ptr::null_mut();
            return start;
        }
        let mut j = 0;
        loop {
            let d = *delim.add(j);
            if d == 0 {
                break;
            }
            if d == c {
                *start.add(i) = 0;
                *s = start.add(i + 1);
                return start;
            }
            j += 1;
        }
        i += 1;
    }
}

/// Linux `memcpy_fromio` — copy from MMIO (volatile reads). In the userspace shim
/// the iomem mapping is plain memory; volatile preserves access ordering/width.
#[no_mangle]
pub unsafe extern "C" fn memcpy_fromio(dst: *mut c_void, src: *const c_void, n: usize) {
    let d = dst as *mut u8;
    let s = src as *const u8;
    let mut i = 0;
    while i < n {
        *d.add(i) = core::ptr::read_volatile(s.add(i));
        i += 1;
    }
}

/// Linux `memcpy_toio` — copy to MMIO (volatile writes).
#[no_mangle]
pub unsafe extern "C" fn memcpy_toio(dst: *mut c_void, src: *const c_void, n: usize) {
    let d = dst as *mut u8;
    let s = src as *const u8;
    let mut i = 0;
    while i < n {
        core::ptr::write_volatile(d.add(i), *s.add(i));
        i += 1;
    }
}

/// Linux `memset_io` — fill MMIO (volatile writes).
#[no_mangle]
pub unsafe extern "C" fn memset_io(dst: *mut c_void, c: i32, n: usize) {
    let d = dst as *mut u8;
    let byte = c as u8;
    let mut i = 0;
    while i < n {
        core::ptr::write_volatile(d.add(i), byte);
        i += 1;
    }
}
