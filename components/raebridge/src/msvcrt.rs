//! MSVC C Runtime Library (CRT) compatibility layer for AthBridge.
//!
//! Provides C standard library functions expected by Windows executables.

#![allow(non_camel_case_types, non_snake_case, dead_code)]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI32, Ordering};

// ---------------------------------------------------------------------------
// errno values
// ---------------------------------------------------------------------------

pub const EPERM: i32 = 1;
pub const ENOENT: i32 = 2;
pub const ESRCH: i32 = 3;
pub const EINTR: i32 = 4;
pub const EIO: i32 = 5;
pub const ENXIO: i32 = 6;
pub const E2BIG: i32 = 7;
pub const ENOEXEC: i32 = 8;
pub const EBADF: i32 = 9;
pub const ECHILD: i32 = 10;
pub const EAGAIN: i32 = 11;
pub const ENOMEM: i32 = 12;
pub const EACCES: i32 = 13;
pub const EFAULT: i32 = 14;
pub const EBUSY: i32 = 16;
pub const EEXIST: i32 = 17;
pub const EXDEV: i32 = 18;
pub const ENODEV: i32 = 19;
pub const ENOTDIR: i32 = 20;
pub const EISDIR: i32 = 21;
pub const EINVAL: i32 = 22;
pub const ENFILE: i32 = 23;
pub const EMFILE: i32 = 24;
pub const ENOTTY: i32 = 25;
pub const EFBIG: i32 = 27;
pub const ENOSPC: i32 = 28;
pub const ESPIPE: i32 = 29;
pub const EROFS: i32 = 30;
pub const EMLINK: i32 = 31;
pub const EPIPE: i32 = 32;
pub const EDOM: i32 = 33;
pub const ERANGE: i32 = 34;
pub const EDEADLK: i32 = 36;
pub const ENAMETOOLONG: i32 = 38;
pub const ENOLCK: i32 = 39;
pub const ENOSYS: i32 = 40;
pub const ENOTEMPTY: i32 = 41;
pub const EILSEQ: i32 = 42;
pub const STRUNCATE: i32 = 80;

static ERRNO: AtomicI32 = AtomicI32::new(0);

pub fn _set_errno(value: i32) {
    ERRNO.store(value, Ordering::Relaxed);
}

pub fn _get_errno() -> i32 {
    ERRNO.load(Ordering::Relaxed)
}

pub fn strerror(errnum: i32) -> &'static str {
    match errnum {
        0 => "No error",
        EPERM => "Operation not permitted",
        ENOENT => "No such file or directory",
        ESRCH => "No such process",
        EINTR => "Interrupted function call",
        EIO => "Input/output error",
        ENXIO => "No such device or address",
        E2BIG => "Argument list too long",
        ENOEXEC => "Exec format error",
        EBADF => "Bad file descriptor",
        ECHILD => "No child processes",
        EAGAIN => "Resource temporarily unavailable",
        ENOMEM => "Not enough space",
        EACCES => "Permission denied",
        EFAULT => "Bad address",
        EBUSY => "Resource busy",
        EEXIST => "File exists",
        EXDEV => "Improper link",
        ENODEV => "No such device",
        ENOTDIR => "Not a directory",
        EISDIR => "Is a directory",
        EINVAL => "Invalid argument",
        ENFILE => "Too many open files in system",
        EMFILE => "Too many open files",
        ENOTTY => "Inappropriate I/O control operation",
        EFBIG => "File too large",
        ENOSPC => "No space left on device",
        ESPIPE => "Invalid seek",
        EROFS => "Read-only file system",
        EMLINK => "Too many links",
        EPIPE => "Broken pipe",
        EDOM => "Domain error",
        ERANGE => "Result too large",
        EDEADLK => "Resource deadlock avoided",
        ENAMETOOLONG => "Filename too long",
        ENOLCK => "No locks available",
        ENOSYS => "Function not implemented",
        ENOTEMPTY => "Directory not empty",
        EILSEQ => "Illegal byte sequence",
        _ => "Unknown error",
    }
}

// ---------------------------------------------------------------------------
// String functions
// ---------------------------------------------------------------------------

pub fn strlen(s: &[u8]) -> usize {
    s.iter().position(|&b| b == 0).unwrap_or(s.len())
}

pub fn strcmp(s1: &[u8], s2: &[u8]) -> i32 {
    let len1 = strlen(s1);
    let len2 = strlen(s2);
    let min_len = if len1 < len2 { len1 } else { len2 };
    for i in 0..min_len {
        if s1[i] != s2[i] {
            return (s1[i] as i32) - (s2[i] as i32);
        }
    }
    (len1 as i32) - (len2 as i32)
}

pub fn strncmp(s1: &[u8], s2: &[u8], n: usize) -> i32 {
    for i in 0..n {
        let c1 = if i < s1.len() { s1[i] } else { 0 };
        let c2 = if i < s2.len() { s2[i] } else { 0 };
        if c1 == 0 && c2 == 0 {
            return 0;
        }
        if c1 != c2 {
            return (c1 as i32) - (c2 as i32);
        }
    }
    0
}

pub fn strcpy(dst: &mut [u8], src: &[u8]) -> usize {
    let len = strlen(src);
    let copy_len = if len + 1 <= dst.len() {
        len
    } else {
        dst.len() - 1
    };
    dst[..copy_len].copy_from_slice(&src[..copy_len]);
    dst[copy_len] = 0;
    copy_len
}

pub fn strncpy(dst: &mut [u8], src: &[u8], n: usize) -> usize {
    let src_len = strlen(src);
    let copy_len = if src_len < n { src_len } else { n };
    let actual = if copy_len <= dst.len() {
        copy_len
    } else {
        dst.len()
    };
    dst[..actual].copy_from_slice(&src[..actual]);
    for i in actual..n.min(dst.len()) {
        dst[i] = 0;
    }
    actual
}

pub fn strcat(dst: &mut [u8], src: &[u8]) -> usize {
    let dst_len = strlen(dst);
    let src_len = strlen(src);
    let remaining = dst.len() - dst_len - 1;
    let copy_len = if src_len < remaining {
        src_len
    } else {
        remaining
    };
    dst[dst_len..dst_len + copy_len].copy_from_slice(&src[..copy_len]);
    dst[dst_len + copy_len] = 0;
    dst_len + copy_len
}

pub fn strncat(dst: &mut [u8], src: &[u8], n: usize) -> usize {
    let dst_len = strlen(dst);
    let src_len = strlen(src);
    let max_copy = if src_len < n { src_len } else { n };
    let remaining = dst.len() - dst_len - 1;
    let copy_len = if max_copy < remaining {
        max_copy
    } else {
        remaining
    };
    dst[dst_len..dst_len + copy_len].copy_from_slice(&src[..copy_len]);
    dst[dst_len + copy_len] = 0;
    dst_len + copy_len
}

pub fn strchr(s: &[u8], c: u8) -> Option<usize> {
    let len = strlen(s);
    for i in 0..len {
        if s[i] == c {
            return Some(i);
        }
    }
    if c == 0 {
        return Some(len);
    }
    None
}

pub fn strrchr(s: &[u8], c: u8) -> Option<usize> {
    let len = strlen(s);
    if c == 0 {
        return Some(len);
    }
    let mut last = None;
    for i in 0..len {
        if s[i] == c {
            last = Some(i);
        }
    }
    last
}

pub fn strstr(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    let h_len = strlen(haystack);
    let n_len = strlen(needle);
    if n_len == 0 {
        return Some(0);
    }
    if n_len > h_len {
        return None;
    }
    for i in 0..=(h_len - n_len) {
        if &haystack[i..i + n_len] == &needle[..n_len] {
            return Some(i);
        }
    }
    None
}

pub fn strpbrk(s: &[u8], accept: &[u8]) -> Option<usize> {
    let s_len = strlen(s);
    let a_len = strlen(accept);
    for i in 0..s_len {
        for j in 0..a_len {
            if s[i] == accept[j] {
                return Some(i);
            }
        }
    }
    None
}

pub fn strspn(s: &[u8], accept: &[u8]) -> usize {
    let s_len = strlen(s);
    let a_len = strlen(accept);
    for i in 0..s_len {
        let mut found = false;
        for j in 0..a_len {
            if s[i] == accept[j] {
                found = true;
                break;
            }
        }
        if !found {
            return i;
        }
    }
    s_len
}

pub fn strcspn(s: &[u8], reject: &[u8]) -> usize {
    let s_len = strlen(s);
    let r_len = strlen(reject);
    for i in 0..s_len {
        for j in 0..r_len {
            if s[i] == reject[j] {
                return i;
            }
        }
    }
    s_len
}

pub fn memcpy(dst: &mut [u8], src: &[u8], n: usize) {
    let copy_len = n.min(dst.len()).min(src.len());
    dst[..copy_len].copy_from_slice(&src[..copy_len]);
}

pub fn memmove(dst: &mut [u8], src: &[u8], n: usize) {
    let copy_len = n.min(dst.len()).min(src.len());
    let tmp: Vec<u8> = src[..copy_len].to_vec();
    dst[..copy_len].copy_from_slice(&tmp);
}

pub fn memset(dst: &mut [u8], val: u8, n: usize) {
    let fill_len = n.min(dst.len());
    for i in 0..fill_len {
        dst[i] = val;
    }
}

pub fn memcmp(s1: &[u8], s2: &[u8], n: usize) -> i32 {
    let cmp_len = n.min(s1.len()).min(s2.len());
    for i in 0..cmp_len {
        if s1[i] != s2[i] {
            return (s1[i] as i32) - (s2[i] as i32);
        }
    }
    0
}

pub fn memchr(s: &[u8], c: u8, n: usize) -> Option<usize> {
    let search_len = n.min(s.len());
    for i in 0..search_len {
        if s[i] == c {
            return Some(i);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Wide string functions
// ---------------------------------------------------------------------------

pub fn wcslen(s: &[u16]) -> usize {
    s.iter().position(|&c| c == 0).unwrap_or(s.len())
}

pub fn wcscmp(s1: &[u16], s2: &[u16]) -> i32 {
    let len1 = wcslen(s1);
    let len2 = wcslen(s2);
    let min_len = if len1 < len2 { len1 } else { len2 };
    for i in 0..min_len {
        if s1[i] != s2[i] {
            return (s1[i] as i32) - (s2[i] as i32);
        }
    }
    (len1 as i32) - (len2 as i32)
}

pub fn wcsncmp(s1: &[u16], s2: &[u16], n: usize) -> i32 {
    for i in 0..n {
        let c1 = if i < s1.len() { s1[i] } else { 0 };
        let c2 = if i < s2.len() { s2[i] } else { 0 };
        if c1 == 0 && c2 == 0 {
            return 0;
        }
        if c1 != c2 {
            return (c1 as i32) - (c2 as i32);
        }
    }
    0
}

pub fn wcscpy(dst: &mut [u16], src: &[u16]) -> usize {
    let len = wcslen(src);
    let copy_len = if len + 1 <= dst.len() {
        len
    } else {
        dst.len() - 1
    };
    dst[..copy_len].copy_from_slice(&src[..copy_len]);
    dst[copy_len] = 0;
    copy_len
}

pub fn wcsncpy(dst: &mut [u16], src: &[u16], n: usize) -> usize {
    let src_len = wcslen(src);
    let copy_len = src_len.min(n).min(dst.len());
    dst[..copy_len].copy_from_slice(&src[..copy_len]);
    for i in copy_len..n.min(dst.len()) {
        dst[i] = 0;
    }
    copy_len
}

pub fn wcscat(dst: &mut [u16], src: &[u16]) -> usize {
    let dst_len = wcslen(dst);
    let src_len = wcslen(src);
    let remaining = dst.len() - dst_len - 1;
    let copy_len = src_len.min(remaining);
    dst[dst_len..dst_len + copy_len].copy_from_slice(&src[..copy_len]);
    dst[dst_len + copy_len] = 0;
    dst_len + copy_len
}

pub fn wcsncat(dst: &mut [u16], src: &[u16], n: usize) -> usize {
    let dst_len = wcslen(dst);
    let src_len = wcslen(src);
    let max_copy = src_len.min(n);
    let remaining = dst.len() - dst_len - 1;
    let copy_len = max_copy.min(remaining);
    dst[dst_len..dst_len + copy_len].copy_from_slice(&src[..copy_len]);
    dst[dst_len + copy_len] = 0;
    dst_len + copy_len
}

pub fn wcschr(s: &[u16], c: u16) -> Option<usize> {
    let len = wcslen(s);
    for i in 0..len {
        if s[i] == c {
            return Some(i);
        }
    }
    if c == 0 {
        return Some(len);
    }
    None
}

pub fn wcsrchr(s: &[u16], c: u16) -> Option<usize> {
    let len = wcslen(s);
    if c == 0 {
        return Some(len);
    }
    let mut last = None;
    for i in 0..len {
        if s[i] == c {
            last = Some(i);
        }
    }
    last
}

pub fn wcsstr(haystack: &[u16], needle: &[u16]) -> Option<usize> {
    let h_len = wcslen(haystack);
    let n_len = wcslen(needle);
    if n_len == 0 {
        return Some(0);
    }
    if n_len > h_len {
        return None;
    }
    for i in 0..=(h_len - n_len) {
        if &haystack[i..i + n_len] == &needle[..n_len] {
            return Some(i);
        }
    }
    None
}

pub fn wmemcpy(dst: &mut [u16], src: &[u16], n: usize) {
    let copy_len = n.min(dst.len()).min(src.len());
    dst[..copy_len].copy_from_slice(&src[..copy_len]);
}

pub fn wmemmove(dst: &mut [u16], src: &[u16], n: usize) {
    let copy_len = n.min(dst.len()).min(src.len());
    let tmp: Vec<u16> = src[..copy_len].to_vec();
    dst[..copy_len].copy_from_slice(&tmp);
}

pub fn wmemset(dst: &mut [u16], val: u16, n: usize) {
    let fill_len = n.min(dst.len());
    for i in 0..fill_len {
        dst[i] = val;
    }
}

pub fn wmemcmp(s1: &[u16], s2: &[u16], n: usize) -> i32 {
    let cmp_len = n.min(s1.len()).min(s2.len());
    for i in 0..cmp_len {
        if s1[i] != s2[i] {
            return (s1[i] as i32) - (s2[i] as i32);
        }
    }
    0
}

// ---------------------------------------------------------------------------
// Conversion functions
// ---------------------------------------------------------------------------

pub fn atoi(s: &[u8]) -> i32 {
    let len = strlen(s);
    if len == 0 {
        return 0;
    }
    let mut i = 0;
    while i < len && (s[i] == b' ' || s[i] == b'\t') {
        i += 1;
    }
    let negative = if i < len && s[i] == b'-' {
        i += 1;
        true
    } else if i < len && s[i] == b'+' {
        i += 1;
        false
    } else {
        false
    };
    let mut result: i32 = 0;
    while i < len && s[i] >= b'0' && s[i] <= b'9' {
        result = result.wrapping_mul(10).wrapping_add((s[i] - b'0') as i32);
        i += 1;
    }
    if negative {
        -result
    } else {
        result
    }
}

pub fn atol(s: &[u8]) -> i64 {
    let len = strlen(s);
    if len == 0 {
        return 0;
    }
    let mut i = 0;
    while i < len && (s[i] == b' ' || s[i] == b'\t') {
        i += 1;
    }
    let negative = if i < len && s[i] == b'-' {
        i += 1;
        true
    } else if i < len && s[i] == b'+' {
        i += 1;
        false
    } else {
        false
    };
    let mut result: i64 = 0;
    while i < len && s[i] >= b'0' && s[i] <= b'9' {
        result = result.wrapping_mul(10).wrapping_add((s[i] - b'0') as i64);
        i += 1;
    }
    if negative {
        -result
    } else {
        result
    }
}

pub fn atof(s: &[u8]) -> f64 {
    let len = strlen(s);
    if len == 0 {
        return 0.0;
    }
    let mut i = 0;
    while i < len && (s[i] == b' ' || s[i] == b'\t') {
        i += 1;
    }
    let negative = if i < len && s[i] == b'-' {
        i += 1;
        true
    } else if i < len && s[i] == b'+' {
        i += 1;
        false
    } else {
        false
    };
    let mut integer_part: f64 = 0.0;
    while i < len && s[i] >= b'0' && s[i] <= b'9' {
        integer_part = integer_part * 10.0 + (s[i] - b'0') as f64;
        i += 1;
    }
    let mut frac_part: f64 = 0.0;
    if i < len && s[i] == b'.' {
        i += 1;
        let mut divisor = 10.0;
        while i < len && s[i] >= b'0' && s[i] <= b'9' {
            frac_part += (s[i] - b'0') as f64 / divisor;
            divisor *= 10.0;
            i += 1;
        }
    }
    let mut result = integer_part + frac_part;
    if i < len && (s[i] == b'e' || s[i] == b'E') {
        i += 1;
        let exp_neg = if i < len && s[i] == b'-' {
            i += 1;
            true
        } else if i < len && s[i] == b'+' {
            i += 1;
            false
        } else {
            false
        };
        let mut exp: i32 = 0;
        while i < len && s[i] >= b'0' && s[i] <= b'9' {
            exp = exp * 10 + (s[i] - b'0') as i32;
            i += 1;
        }
        if exp_neg {
            exp = -exp;
        }
        result *= libm::pow(10.0, exp as f64);
    }
    if negative {
        -result
    } else {
        result
    }
}

pub fn strtol(s: &[u8], base: u32) -> (i64, usize) {
    let len = strlen(s);
    let mut i = 0;
    while i < len && (s[i] == b' ' || s[i] == b'\t') {
        i += 1;
    }
    let negative = if i < len && s[i] == b'-' {
        i += 1;
        true
    } else if i < len && s[i] == b'+' {
        i += 1;
        false
    } else {
        false
    };
    let actual_base = if base == 0 {
        if i + 1 < len && s[i] == b'0' && (s[i + 1] == b'x' || s[i + 1] == b'X') {
            i += 2;
            16u32
        } else if i < len && s[i] == b'0' {
            8
        } else {
            10
        }
    } else {
        if base == 16 && i + 1 < len && s[i] == b'0' && (s[i + 1] == b'x' || s[i + 1] == b'X') {
            i += 2;
        }
        base
    };
    let mut result: i64 = 0;
    let start = i;
    while i < len {
        let digit = match s[i] {
            b'0'..=b'9' => (s[i] - b'0') as u32,
            b'a'..=b'f' => (s[i] - b'a' + 10) as u32,
            b'A'..=b'F' => (s[i] - b'A' + 10) as u32,
            _ => break,
        };
        if digit >= actual_base {
            break;
        }
        result = result
            .wrapping_mul(actual_base as i64)
            .wrapping_add(digit as i64);
        i += 1;
    }
    if i == start {
        return (0, 0);
    }
    (if negative { -result } else { result }, i)
}

pub fn strtoul(s: &[u8], base: u32) -> (u64, usize) {
    let (val, consumed) = strtol(s, base);
    (val as u64, consumed)
}

pub fn strtod(s: &[u8]) -> (f64, usize) {
    let val = atof(s);
    let len = strlen(s);
    (val, len)
}

pub fn itoa(value: i32, buf: &mut [u8], radix: u32) -> usize {
    if buf.is_empty() {
        return 0;
    }
    let negative = value < 0 && radix == 10;
    let mut v = if negative {
        (-(value as i64)) as u64
    } else {
        value as u64
    };
    let mut tmp = [0u8; 34];
    let mut pos = 0;
    if v == 0 {
        tmp[0] = b'0';
        pos = 1;
    } else {
        while v > 0 {
            let digit = (v % radix as u64) as u8;
            tmp[pos] = if digit < 10 {
                b'0' + digit
            } else {
                b'a' + digit - 10
            };
            v /= radix as u64;
            pos += 1;
        }
    }
    let total = pos + if negative { 1 } else { 0 };
    if total >= buf.len() {
        return 0;
    }
    let mut idx = 0;
    if negative {
        buf[idx] = b'-';
        idx += 1;
    }
    for i in (0..pos).rev() {
        buf[idx] = tmp[i];
        idx += 1;
    }
    buf[idx] = 0;
    idx
}

// ---------------------------------------------------------------------------
// Memory allocation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct HeapBlock {
    ptr: u64,
    size: usize,
    alignment: usize,
    allocated: bool,
}

pub struct CrtHeap {
    blocks: Vec<HeapBlock>,
    next_addr: u64,
    total_allocated: usize,
    total_freed: usize,
    alloc_count: u64,
    debug_flags: u32,
    break_alloc: Option<u64>,
}

impl CrtHeap {
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            next_addr: 0x10000000,
            total_allocated: 0,
            total_freed: 0,
            alloc_count: 0,
            debug_flags: 0,
            break_alloc: None,
        }
    }

    pub fn malloc(&mut self, size: usize) -> u64 {
        if size == 0 {
            return 0;
        }
        self.alloc_count += 1;
        if let Some(break_id) = self.break_alloc {
            if self.alloc_count == break_id {
                return 0; // simulate failure at break point
            }
        }
        let ptr = self.next_addr;
        let aligned_size = (size + 15) & !15;
        self.next_addr += aligned_size as u64;
        self.total_allocated += size;
        self.blocks.push(HeapBlock {
            ptr,
            size,
            alignment: 16,
            allocated: true,
        });
        ptr
    }

    pub fn calloc(&mut self, count: usize, size: usize) -> u64 {
        let total = count.saturating_mul(size);
        self.malloc(total)
    }

    pub fn realloc(&mut self, ptr: u64, new_size: usize) -> u64 {
        if ptr == 0 {
            return self.malloc(new_size);
        }
        if new_size == 0 {
            self.free(ptr);
            return 0;
        }
        if let Some(block) = self.blocks.iter_mut().find(|b| b.ptr == ptr && b.allocated) {
            if new_size <= block.size {
                block.size = new_size;
                return ptr;
            }
            block.allocated = false;
            self.total_freed += block.size;
        }
        self.malloc(new_size)
    }

    pub fn free(&mut self, ptr: u64) {
        if ptr == 0 {
            return;
        }
        if let Some(block) = self.blocks.iter_mut().find(|b| b.ptr == ptr && b.allocated) {
            block.allocated = false;
            self.total_freed += block.size;
        }
    }

    pub fn _msize(&self, ptr: u64) -> Option<usize> {
        self.blocks
            .iter()
            .find(|b| b.ptr == ptr && b.allocated)
            .map(|b| b.size)
    }

    pub fn _aligned_malloc(&mut self, size: usize, alignment: usize) -> u64 {
        if size == 0 || alignment == 0 {
            return 0;
        }
        let align_mask = alignment - 1;
        let aligned_addr = (self.next_addr as usize + align_mask) & !align_mask;
        let ptr = aligned_addr as u64;
        let padded_size = (size + alignment - 1) & !(alignment - 1);
        self.next_addr = ptr + padded_size as u64;
        self.total_allocated += size;
        self.alloc_count += 1;
        self.blocks.push(HeapBlock {
            ptr,
            size,
            alignment,
            allocated: true,
        });
        ptr
    }

    pub fn _aligned_free(&mut self, ptr: u64) {
        self.free(ptr);
    }

    pub fn _aligned_realloc(&mut self, ptr: u64, size: usize, alignment: usize) -> u64 {
        if ptr == 0 {
            return self._aligned_malloc(size, alignment);
        }
        self._aligned_free(ptr);
        self._aligned_malloc(size, alignment)
    }

    pub fn check_memory(&self) -> bool {
        self.blocks.iter().all(|b| b.ptr != 0)
    }

    pub fn dump_leaks(&self) -> Vec<(u64, usize)> {
        self.blocks
            .iter()
            .filter(|b| b.allocated)
            .map(|b| (b.ptr, b.size))
            .collect()
    }

    pub fn set_debug_flags(&mut self, flags: u32) {
        self.debug_flags = flags;
    }
    pub fn set_break_alloc(&mut self, alloc_id: u64) {
        self.break_alloc = Some(alloc_id);
    }
    pub fn total_allocated(&self) -> usize {
        self.total_allocated
    }
    pub fn total_freed(&self) -> usize {
        self.total_freed
    }
}

// ---------------------------------------------------------------------------
// Math functions (using libm)
// ---------------------------------------------------------------------------

pub fn sin(x: f64) -> f64 {
    libm::sin(x)
}
pub fn cos(x: f64) -> f64 {
    libm::cos(x)
}
pub fn tan(x: f64) -> f64 {
    libm::tan(x)
}
pub fn asin(x: f64) -> f64 {
    libm::asin(x)
}
pub fn acos(x: f64) -> f64 {
    libm::acos(x)
}
pub fn atan(x: f64) -> f64 {
    libm::atan(x)
}
pub fn atan2(y: f64, x: f64) -> f64 {
    libm::atan2(y, x)
}
pub fn sinh(x: f64) -> f64 {
    libm::sinh(x)
}
pub fn cosh(x: f64) -> f64 {
    libm::cosh(x)
}
pub fn tanh(x: f64) -> f64 {
    libm::tanh(x)
}
pub fn exp(x: f64) -> f64 {
    libm::exp(x)
}
pub fn log(x: f64) -> f64 {
    libm::log(x)
}
pub fn log2(x: f64) -> f64 {
    libm::log2(x)
}
pub fn log10(x: f64) -> f64 {
    libm::log10(x)
}
pub fn pow(base: f64, exp_val: f64) -> f64 {
    libm::pow(base, exp_val)
}
pub fn sqrt(x: f64) -> f64 {
    libm::sqrt(x)
}
pub fn ceil(x: f64) -> f64 {
    libm::ceil(x)
}
pub fn floor(x: f64) -> f64 {
    libm::floor(x)
}
pub fn round(x: f64) -> f64 {
    libm::round(x)
}
pub fn trunc(x: f64) -> f64 {
    libm::trunc(x)
}
pub fn fabs(x: f64) -> f64 {
    libm::fabs(x)
}
pub fn fmod(x: f64, y: f64) -> f64 {
    libm::fmod(x, y)
}
pub fn copysign(x: f64, y: f64) -> f64 {
    libm::copysign(x, y)
}
pub fn hypot(x: f64, y: f64) -> f64 {
    libm::hypot(x, y)
}
pub fn cbrt(x: f64) -> f64 {
    libm::cbrt(x)
}
pub fn erf(x: f64) -> f64 {
    libm::erf(x)
}

pub fn modf(x: f64) -> (f64, f64) {
    let i = libm::trunc(x);
    (x - i, i)
}

pub fn frexp(x: f64) -> (f64, i32) {
    libm::frexp(x)
}

pub fn ldexp(x: f64, n: i32) -> f64 {
    libm::ldexp(x, n)
}

pub fn isnan(x: f64) -> bool {
    x != x
}
pub fn isinf(x: f64) -> bool {
    x == f64::INFINITY || x == f64::NEG_INFINITY
}

pub const INFINITY: f64 = f64::INFINITY;
pub const NAN: f64 = f64::NAN;

// ---------------------------------------------------------------------------
// File I/O abstraction
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileMode {
    Read,
    Write,
    Append,
    ReadWrite,
    ReadWriteCreate,
    AppendRead,
}

impl FileMode {
    pub fn from_str(mode: &[u8]) -> Option<Self> {
        let m = strlen(mode);
        if m == 0 {
            return None;
        }
        match (
            mode[0],
            if m > 1 { mode[1] } else { 0 },
            if m > 2 { mode[2] } else { 0 },
        ) {
            (b'r', 0, _) => Some(Self::Read),
            (b'r', b'+', _) => Some(Self::ReadWrite),
            (b'w', 0, _) => Some(Self::Write),
            (b'w', b'+', _) => Some(Self::ReadWriteCreate),
            (b'a', 0, _) => Some(Self::Append),
            (b'a', b'+', _) => Some(Self::AppendRead),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum SeekOrigin {
    Set = 0,
    Cur = 1,
    End = 2,
}

#[derive(Debug)]
pub struct CrtFile {
    pub id: u64,
    pub path: String,
    pub mode: FileMode,
    pub data: Vec<u8>,
    pub position: usize,
    pub eof: bool,
    pub error: bool,
    pub unget_char: Option<u8>,
}

impl CrtFile {
    pub fn new(id: u64, path: String, mode: FileMode) -> Self {
        Self {
            id,
            path,
            mode,
            data: Vec::new(),
            position: 0,
            eof: false,
            error: false,
            unget_char: None,
        }
    }

    pub fn fread(&mut self, buf: &mut [u8], size: usize, count: usize) -> usize {
        let total = size * count;
        let available = self.data.len().saturating_sub(self.position);
        let to_read = total.min(available).min(buf.len());
        if to_read == 0 {
            self.eof = true;
            return 0;
        }
        buf[..to_read].copy_from_slice(&self.data[self.position..self.position + to_read]);
        self.position += to_read;
        if self.position >= self.data.len() {
            self.eof = true;
        }
        to_read / size
    }

    pub fn fwrite(&mut self, buf: &[u8], size: usize, count: usize) -> usize {
        let total = size * count;
        let to_write = total.min(buf.len());
        if self.position >= self.data.len() {
            self.data.extend_from_slice(&buf[..to_write]);
        } else {
            let end = self.position + to_write;
            if end > self.data.len() {
                self.data.resize(end, 0);
            }
            self.data[self.position..end].copy_from_slice(&buf[..to_write]);
        }
        self.position += to_write;
        to_write / size
    }

    pub fn fseek(&mut self, offset: i64, origin: SeekOrigin) -> i32 {
        let new_pos = match origin {
            SeekOrigin::Set => offset as usize,
            SeekOrigin::Cur => (self.position as i64 + offset) as usize,
            SeekOrigin::End => (self.data.len() as i64 + offset) as usize,
        };
        self.position = new_pos;
        self.eof = false;
        0
    }

    pub fn ftell(&self) -> i64 {
        self.position as i64
    }
    pub fn feof(&self) -> bool {
        self.eof
    }
    pub fn ferror(&self) -> bool {
        self.error
    }

    pub fn fflush(&mut self) -> i32 {
        0
    }

    pub fn fgetc(&mut self) -> Option<u8> {
        if let Some(c) = self.unget_char.take() {
            return Some(c);
        }
        if self.position >= self.data.len() {
            self.eof = true;
            return None;
        }
        let c = self.data[self.position];
        self.position += 1;
        Some(c)
    }

    pub fn fputc(&mut self, c: u8) -> i32 {
        if self.position >= self.data.len() {
            self.data.push(c);
        } else {
            self.data[self.position] = c;
        }
        self.position += 1;
        c as i32
    }

    pub fn ungetc(&mut self, c: u8) -> i32 {
        self.unget_char = Some(c);
        self.eof = false;
        c as i32
    }

    pub fn fgets(&mut self, buf: &mut [u8]) -> Option<usize> {
        if self.eof || buf.is_empty() {
            return None;
        }
        let max = buf.len() - 1;
        let mut written = 0;
        while written < max {
            let c = self.fgetc()?;
            buf[written] = c;
            written += 1;
            if c == b'\n' {
                break;
            }
        }
        buf[written] = 0;
        Some(written)
    }

    pub fn fputs(&mut self, s: &[u8]) -> i32 {
        let len = strlen(s);
        for i in 0..len {
            self.fputc(s[i]);
        }
        len as i32
    }
}

// ---------------------------------------------------------------------------
// Printf format parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatSpec {
    Decimal,
    Integer,
    Unsigned,
    Hex,
    HexUpper,
    Octal,
    String,
    WideString,
    Char,
    WideChar,
    Float,
    Scientific,
    ScientificUpper,
    General,
    GeneralUpper,
    Pointer,
    Count,
    Percent,
}

#[derive(Debug, Clone, Copy)]
pub enum LengthModifier {
    None,
    Short,
    ShortShort,
    Long,
    LongLong,
    SizeT,
    IntMax,
    PtrDiff,
}

#[derive(Debug, Clone, Copy)]
pub struct FormatFlags {
    pub left_justify: bool,
    pub force_sign: bool,
    pub space_sign: bool,
    pub hash: bool,
    pub zero_pad: bool,
}

impl Default for FormatFlags {
    fn default() -> Self {
        Self {
            left_justify: false,
            force_sign: false,
            space_sign: false,
            hash: false,
            zero_pad: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FormatDirective {
    pub flags: FormatFlags,
    pub width: Option<u32>,
    pub precision: Option<u32>,
    pub length: LengthModifier,
    pub spec: FormatSpec,
}

pub fn parse_format_string(fmt: &[u8]) -> Vec<FormatDirective> {
    let mut directives = Vec::new();
    let len = strlen(fmt);
    let mut i = 0;

    while i < len {
        if fmt[i] != b'%' {
            i += 1;
            continue;
        }
        i += 1;
        if i >= len {
            break;
        }
        if fmt[i] == b'%' {
            i += 1;
            directives.push(FormatDirective {
                flags: FormatFlags::default(),
                width: None,
                precision: None,
                length: LengthModifier::None,
                spec: FormatSpec::Percent,
            });
            continue;
        }

        let mut flags = FormatFlags::default();
        loop {
            if i >= len {
                break;
            }
            match fmt[i] {
                b'-' => {
                    flags.left_justify = true;
                    i += 1;
                }
                b'+' => {
                    flags.force_sign = true;
                    i += 1;
                }
                b' ' => {
                    flags.space_sign = true;
                    i += 1;
                }
                b'#' => {
                    flags.hash = true;
                    i += 1;
                }
                b'0' => {
                    flags.zero_pad = true;
                    i += 1;
                }
                _ => break,
            }
        }

        let mut width = None;
        if i < len && fmt[i] >= b'0' && fmt[i] <= b'9' {
            let mut w = 0u32;
            while i < len && fmt[i] >= b'0' && fmt[i] <= b'9' {
                w = w * 10 + (fmt[i] - b'0') as u32;
                i += 1;
            }
            width = Some(w);
        } else if i < len && fmt[i] == b'*' {
            width = Some(0);
            i += 1;
        }

        let mut precision = None;
        if i < len && fmt[i] == b'.' {
            i += 1;
            if i < len && fmt[i] >= b'0' && fmt[i] <= b'9' {
                let mut p = 0u32;
                while i < len && fmt[i] >= b'0' && fmt[i] <= b'9' {
                    p = p * 10 + (fmt[i] - b'0') as u32;
                    i += 1;
                }
                precision = Some(p);
            } else if i < len && fmt[i] == b'*' {
                precision = Some(0);
                i += 1;
            } else {
                precision = Some(0);
            }
        }

        let mut length = LengthModifier::None;
        if i < len {
            match fmt[i] {
                b'h' => {
                    i += 1;
                    if i < len && fmt[i] == b'h' {
                        length = LengthModifier::ShortShort;
                        i += 1;
                    } else {
                        length = LengthModifier::Short;
                    }
                }
                b'l' => {
                    i += 1;
                    if i < len && fmt[i] == b'l' {
                        length = LengthModifier::LongLong;
                        i += 1;
                    } else {
                        length = LengthModifier::Long;
                    }
                }
                b'z' => {
                    length = LengthModifier::SizeT;
                    i += 1;
                }
                b'j' => {
                    length = LengthModifier::IntMax;
                    i += 1;
                }
                b't' => {
                    length = LengthModifier::PtrDiff;
                    i += 1;
                }
                b'I' => {
                    i += 1;
                    if i + 1 < len && fmt[i] == b'6' && fmt[i + 1] == b'4' {
                        length = LengthModifier::LongLong;
                        i += 2;
                    } else if i + 1 < len && fmt[i] == b'3' && fmt[i + 1] == b'2' {
                        i += 2;
                    }
                }
                _ => {}
            }
        }

        if i >= len {
            break;
        }
        let spec = match fmt[i] {
            b'd' | b'i' => FormatSpec::Decimal,
            b'u' => FormatSpec::Unsigned,
            b'x' => FormatSpec::Hex,
            b'X' => FormatSpec::HexUpper,
            b'o' => FormatSpec::Octal,
            b's' => FormatSpec::String,
            b'S' => FormatSpec::WideString,
            b'c' => FormatSpec::Char,
            b'C' => FormatSpec::WideChar,
            b'f' | b'F' => FormatSpec::Float,
            b'e' => FormatSpec::Scientific,
            b'E' => FormatSpec::ScientificUpper,
            b'g' => FormatSpec::General,
            b'G' => FormatSpec::GeneralUpper,
            b'p' => FormatSpec::Pointer,
            b'n' => FormatSpec::Count,
            _ => FormatSpec::Decimal,
        };
        i += 1;

        directives.push(FormatDirective {
            flags,
            width,
            precision,
            length,
            spec,
        });
    }
    directives
}

// ---------------------------------------------------------------------------
// Vararg C format engine (printf family) — consumes a win64 va_list
// ---------------------------------------------------------------------------
//
// The MSVC CRT routes every formatted-output function (`printf`, `sprintf`,
// `_snprintf`, `fwprintf`, ... and the UCRT `__stdio_common_v*printf` core)
// through one variadic formatter. `parse_format_string` above recognises the
// grammar but discards the literal text between directives and never consumes
// arguments, so a shim built on it would emit the raw format string — exactly
// the "leaks `%d` instead of the number" bug. `format_va` is the real engine:
// a single left-to-right pass that interleaves literals and directives and
// pulls each argument from the win64 va_list in order.
//
// win64 variadic ABI: after the named parameters, every argument occupies one
// 8-byte stack slot — integers sign/zero-extended, `float` promoted to `double`
// and stored as its 8-byte bit pattern, pointers as 8-byte VAs. So a single
// "give me the next 8 bytes" closure (`next`) is a faithful `va_arg`.

/// Upper bound on a `%s`/`%ls` scan so a missing NUL in a guest string can never
/// spin unbounded (1 MiB of characters is far past any real format argument).
const FMT_MAX_STR: usize = 1 << 20;

/// Read a NUL-terminated narrow guest string at `ptr` (guest VA == host VA),
/// bounded by [`FMT_MAX_STR`]. `ptr == 0` yields the CRT's `"(null)"`.
///
/// SAFETY: only reached through a runtime shim where `ptr` is a live guest VA;
/// host KATs pass pointers into live local buffers.
unsafe fn read_c_str_lossy(ptr: u64, max: usize) -> String {
    if ptr == 0 {
        return String::from("(null)");
    }
    let mut s = String::new();
    let limit = max.min(FMT_MAX_STR);
    for i in 0..limit {
        let b = core::ptr::read((ptr + i as u64) as *const u8);
        if b == 0 {
            break;
        }
        s.push(b as char); // Latin-1 → char (ASCII-faithful, never lossy round-trip)
    }
    s
}

/// Read a NUL-terminated wide (UTF-16) guest string at `ptr`, bounded.
///
/// SAFETY: as [`read_c_str_lossy`].
unsafe fn read_wide_str_lossy(ptr: u64, max: usize) -> String {
    if ptr == 0 {
        return String::from("(null)");
    }
    let mut units: Vec<u16> = Vec::new();
    let limit = max.min(FMT_MAX_STR);
    for i in 0..limit {
        let u = core::ptr::read((ptr + (i * 2) as u64) as *const u16);
        if u == 0 {
            break;
        }
        units.push(u);
    }
    String::from_utf16_lossy(&units)
}

/// Format an unsigned integer `v` in `base` (8/10/16) into an ASCII digit
/// string with no sign and no prefix. `upper` selects `A-F` vs `a-f`.
fn fmt_uint_digits(mut v: u64, base: u64, upper: bool) -> String {
    if v == 0 {
        return String::from("0");
    }
    let digits: &[u8] = if upper {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };
    let mut tmp = [0u8; 64];
    let mut n = 0;
    while v > 0 {
        tmp[n] = digits[(v % base) as usize];
        v /= base;
        n += 1;
    }
    let mut s = String::with_capacity(n);
    for i in (0..n).rev() {
        s.push(tmp[i] as char);
    }
    s
}

/// Assemble one converted field: `prefix` (sign or `0x`) + `body` (digits/text),
/// applying `precision` as a minimum digit count for numerics (`numeric_zeropad`
/// true) and then padding to `width` per the flags. This is the shared tail for
/// every directive so width/justify/zero-fill behave identically everywhere.
fn assemble_field(
    out: &mut String,
    prefix: &str,
    body: &str,
    width: usize,
    flags: &FormatFlags,
    numeric_zeropad: bool,
) {
    let total = prefix.len() + body.len();
    if total >= width {
        out.push_str(prefix);
        out.push_str(body);
        return;
    }
    let pad = width - total;
    if flags.left_justify {
        out.push_str(prefix);
        out.push_str(body);
        for _ in 0..pad {
            out.push(' ');
        }
    } else if flags.zero_pad && numeric_zeropad {
        out.push_str(prefix);
        for _ in 0..pad {
            out.push('0');
        }
        out.push_str(body);
    } else {
        for _ in 0..pad {
            out.push(' ');
        }
        out.push_str(prefix);
        out.push_str(body);
    }
}

/// Format a **finite** `f64` in fixed (`'f'`) notation to `prec` fractional
/// digits, magnitude only (no sign). Uses `alloc::format!`'s correct
/// round-half-to-even. Callers guard `inf`/`nan` before reaching here.
fn fmt_float_fixed(v: f64, prec: usize) -> String {
    alloc::format!("{:.*}", prec, v)
}

/// The variadic C formatter. See module section header for the ABI contract.
pub fn format_va(fmt: &[u8], next: &mut dyn FnMut() -> u64, wide_default: bool) -> String {
    let len = strlen(fmt);
    let mut out = String::new();
    let mut i = 0usize;

    while i < len {
        let c = fmt[i];
        if c != b'%' {
            out.push(c as char);
            i += 1;
            continue;
        }
        i += 1;
        if i >= len {
            break;
        }
        if fmt[i] == b'%' {
            out.push('%');
            i += 1;
            continue;
        }

        // Flags.
        let mut flags = FormatFlags::default();
        loop {
            if i >= len {
                break;
            }
            match fmt[i] {
                b'-' => flags.left_justify = true,
                b'+' => flags.force_sign = true,
                b' ' => flags.space_sign = true,
                b'#' => flags.hash = true,
                b'0' => flags.zero_pad = true,
                _ => break,
            }
            i += 1;
        }

        // Width (number or `*` → pulls an int arg; negative = left-justify).
        let mut width: usize = 0;
        if i < len && fmt[i] == b'*' {
            let w = next() as i32;
            if w < 0 {
                flags.left_justify = true;
                width = (-(w as i64)) as usize;
            } else {
                width = w as usize;
            }
            i += 1;
        } else {
            while i < len && fmt[i].is_ascii_digit() {
                width = width * 10 + (fmt[i] - b'0') as usize;
                i += 1;
            }
        }

        // Precision (`.` then number or `*`; bare `.` == 0).
        let mut precision: Option<usize> = None;
        if i < len && fmt[i] == b'.' {
            i += 1;
            if i < len && fmt[i] == b'*' {
                let p = next() as i32;
                precision = Some(if p < 0 { 0 } else { p as usize });
                i += 1;
            } else {
                let mut p = 0usize;
                while i < len && fmt[i].is_ascii_digit() {
                    p = p * 10 + (fmt[i] - b'0') as usize;
                    i += 1;
                }
                precision = Some(p);
            }
        }

        // Length modifier.
        let mut length = LengthModifier::None;
        if i < len {
            match fmt[i] {
                b'h' => {
                    i += 1;
                    if i < len && fmt[i] == b'h' {
                        length = LengthModifier::ShortShort;
                        i += 1;
                    } else {
                        length = LengthModifier::Short;
                    }
                }
                b'l' => {
                    i += 1;
                    if i < len && fmt[i] == b'l' {
                        length = LengthModifier::LongLong;
                        i += 1;
                    } else {
                        length = LengthModifier::Long;
                    }
                }
                b'w' => {
                    length = LengthModifier::Long; // MSVC `%ws` == wide string
                    i += 1;
                }
                b'z' => {
                    length = LengthModifier::SizeT;
                    i += 1;
                }
                b'j' => {
                    length = LengthModifier::IntMax;
                    i += 1;
                }
                b't' => {
                    length = LengthModifier::PtrDiff;
                    i += 1;
                }
                b'L' => {
                    length = LengthModifier::LongLong;
                    i += 1;
                }
                b'I' => {
                    i += 1;
                    if i + 1 < len && fmt[i] == b'6' && fmt[i + 1] == b'4' {
                        length = LengthModifier::LongLong;
                        i += 2;
                    } else if i + 1 < len && fmt[i] == b'3' && fmt[i + 1] == b'2' {
                        i += 2;
                    }
                }
                _ => {}
            }
        }

        if i >= len {
            break;
        }
        let conv = fmt[i];
        i += 1;

        // Width of the integer slot for signed/unsigned masking.
        let is_wide_len = matches!(length, LengthModifier::Long);
        let is_64 = matches!(
            length,
            LengthModifier::LongLong
                | LengthModifier::SizeT
                | LengthModifier::IntMax
                | LengthModifier::PtrDiff
        );

        match conv {
            b'd' | b'i' => {
                let raw = next();
                let val: i64 = if is_64 {
                    raw as i64
                } else {
                    match length {
                        LengthModifier::ShortShort => (raw as u8) as i8 as i64,
                        LengthModifier::Short => (raw as u16) as i16 as i64,
                        _ => (raw as u32) as i32 as i64,
                    }
                };
                let neg = val < 0;
                let mag = if neg {
                    (val as i128).unsigned_abs() as u64
                } else {
                    val as u64
                };
                let mut body = fmt_uint_digits(mag, 10, false);
                if let Some(p) = precision {
                    if body.len() < p {
                        let zeros = p - body.len();
                        let mut z = String::with_capacity(p);
                        for _ in 0..zeros {
                            z.push('0');
                        }
                        z.push_str(&body);
                        body = z;
                    }
                    if p == 0 && mag == 0 {
                        body.clear();
                    }
                }
                let prefix = if neg {
                    "-"
                } else if flags.force_sign {
                    "+"
                } else if flags.space_sign {
                    " "
                } else {
                    ""
                };
                let zeropad = precision.is_none();
                assemble_field(&mut out, prefix, &body, width, &flags, zeropad);
            }
            b'u' | b'x' | b'X' | b'o' => {
                let raw = next();
                let val: u64 = if is_64 {
                    raw
                } else {
                    match length {
                        LengthModifier::ShortShort => (raw as u8) as u64,
                        LengthModifier::Short => (raw as u16) as u64,
                        _ => (raw as u32) as u64,
                    }
                };
                let (base, upper): (u64, bool) = match conv {
                    b'x' => (16, false),
                    b'X' => (16, true),
                    b'o' => (8, false),
                    _ => (10, false),
                };
                let mut body = fmt_uint_digits(val, base, upper);
                if let Some(p) = precision {
                    if body.len() < p {
                        let zeros = p - body.len();
                        let mut z = String::with_capacity(p);
                        for _ in 0..zeros {
                            z.push('0');
                        }
                        z.push_str(&body);
                        body = z;
                    }
                    if p == 0 && val == 0 {
                        body.clear();
                    }
                }
                let prefix = if flags.hash && val != 0 {
                    match conv {
                        b'x' => "0x",
                        b'X' => "0X",
                        b'o' => "0",
                        _ => "",
                    }
                } else {
                    ""
                };
                let zeropad = precision.is_none();
                assemble_field(&mut out, prefix, &body, width, &flags, zeropad);
            }
            b'c' => {
                let raw = next();
                let ch = if wide_default || is_wide_len {
                    char::from_u32(raw as u32 & 0xFFFF).unwrap_or('\u{FFFD}')
                } else {
                    (raw as u8) as char
                };
                let mut body = String::new();
                body.push(ch);
                assemble_field(&mut out, "", &body, width, &flags, false);
            }
            b'C' => {
                let raw = next();
                // `%C` inverts the default width of `%c`.
                let ch = if wide_default {
                    (raw as u8) as char
                } else {
                    char::from_u32(raw as u32 & 0xFFFF).unwrap_or('\u{FFFD}')
                };
                let mut body = String::new();
                body.push(ch);
                assemble_field(&mut out, "", &body, width, &flags, false);
            }
            b's' | b'S' => {
                let ptr = next();
                // `%s` follows `wide_default`; `%ls`/`%ws` force wide; `%hs`
                // forces narrow; `%S` inverts the family default.
                let wide = match (conv, length) {
                    (_, LengthModifier::Long) => true,
                    (_, LengthModifier::Short) | (_, LengthModifier::ShortShort) => false,
                    (b'S', _) => !wide_default,
                    _ => wide_default,
                };
                let max = precision.unwrap_or(FMT_MAX_STR);
                // SAFETY: runtime shim passes a live guest VA; KATs pass live
                // host buffers. Bounded by `max`/FMT_MAX_STR.
                let mut s = unsafe {
                    if wide {
                        read_wide_str_lossy(ptr, max)
                    } else {
                        read_c_str_lossy(ptr, max)
                    }
                };
                if let Some(p) = precision {
                    if s.chars().count() > p {
                        s = s.chars().take(p).collect();
                    }
                }
                assemble_field(&mut out, "", &s, width, &flags, false);
            }
            b'p' => {
                let raw = next();
                // MSVC `%p`: uppercase hex, zero-padded to pointer width (16).
                let body = fmt_uint_digits(raw, 16, true);
                let mut padded = String::new();
                for _ in body.len()..16 {
                    padded.push('0');
                }
                padded.push_str(&body);
                assemble_field(&mut out, "", &padded, width, &flags, false);
            }
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' => {
                let bits = next();
                let v = f64::from_bits(bits);
                let prec = precision.unwrap_or(6);
                let upper = conv.is_ascii_uppercase();
                let neg = v.is_sign_negative() && !v.is_nan();
                let mag = if neg { -v } else { v };
                let body = if v.is_nan() {
                    if upper {
                        String::from("NAN")
                    } else {
                        String::from("nan")
                    }
                } else if v.is_infinite() {
                    if upper {
                        String::from("INF")
                    } else {
                        String::from("inf")
                    }
                } else {
                    match conv.to_ascii_lowercase() {
                        b'f' => fmt_float_fixed(mag, prec),
                        b'e' => fmt_float_sci(mag, prec, upper),
                        _ => fmt_float_general(mag, prec, upper),
                    }
                };
                let non_finite = v.is_nan() || v.is_infinite();
                let prefix = if neg {
                    "-"
                } else if flags.force_sign {
                    "+"
                } else if flags.space_sign {
                    " "
                } else {
                    ""
                };
                let zeropad = !non_finite;
                assemble_field(&mut out, prefix, &body, width, &flags, zeropad);
            }
            b'n' => {
                // `%n` is disabled by default in the MSVC CRT (security). Consume
                // the pointer argument and write nothing — matching that default.
                let _ = next();
            }
            other => {
                // Unknown conversion: emit it verbatim (`%` already dropped).
                out.push('%');
                out.push(other as char);
            }
        }
    }
    out
}

/// `'e'`-style scientific: `d.ddde[+-]XX`, `prec` fractional digits, exponent
/// at least two digits (C/MSVC rule). Magnitude only.
fn fmt_float_sci(v: f64, prec: usize, upper: bool) -> String {
    if v == 0.0 {
        let mantissa = if prec == 0 {
            String::from("0")
        } else {
            let mut s = String::from("0.");
            for _ in 0..prec {
                s.push('0');
            }
            s
        };
        return alloc::format!("{}{}+00", mantissa, if upper { 'E' } else { 'e' });
    }
    // Normalise to [1,10).
    let mut exp = 0i32;
    let mut m = v;
    while m >= 10.0 {
        m /= 10.0;
        exp += 1;
    }
    while m < 1.0 {
        m *= 10.0;
        exp -= 1;
    }
    // Round the mantissa at `prec`; a carry can bump it to 10.0.
    let mut mant = alloc::format!("{:.*}", prec, m);
    if mant.starts_with("10") {
        m /= 10.0;
        exp += 1;
        mant = alloc::format!("{:.*}", prec, m);
    }
    let esign = if exp < 0 { '-' } else { '+' };
    let eabs = exp.unsigned_abs();
    alloc::format!(
        "{}{}{}{:02}",
        mant,
        if upper { 'E' } else { 'e' },
        esign,
        eabs
    )
}

/// `'g'`-style: pick `%e` when the exponent is `< -4` or `>= precision`, else
/// `%f`; trailing zeros trimmed (no `#` flag). `prec` 0 is treated as 1 (C rule).
fn fmt_float_general(v: f64, prec: usize, upper: bool) -> String {
    let p = if prec == 0 { 1 } else { prec };
    // Determine decimal exponent.
    let mut exp = 0i32;
    if v != 0.0 {
        let mut m = v;
        while m >= 10.0 {
            m /= 10.0;
            exp += 1;
        }
        while m < 1.0 {
            m *= 10.0;
            exp -= 1;
        }
    }
    let mut s = if exp < -4 || exp >= p as i32 {
        fmt_float_sci(v, p.saturating_sub(1), upper)
    } else {
        let frac = (p as i32 - 1 - exp).max(0) as usize;
        fmt_float_fixed(v, frac)
    };
    // Trim trailing zeros in the fractional part (and a bare trailing '.').
    if s.contains('.') {
        let (mant, tail) = split_exponent(&s, upper);
        let trimmed = {
            let mut m = mant.trim_end_matches('0').to_string();
            if m.ends_with('.') {
                m.pop();
            }
            m
        };
        s = alloc::format!("{}{}", trimmed, tail);
    }
    s
}

/// Split a scientific string into `(mantissa, "eXX")`; a fixed string returns
/// `(whole, "")`. Used by `%g` trailing-zero trimming.
fn split_exponent(s: &str, upper: bool) -> (String, String) {
    let e = if upper { 'E' } else { 'e' };
    if let Some(pos) = s.find(e) {
        (s[..pos].to_string(), s[pos..].to_string())
    } else {
        (s.to_string(), String::new())
    }
}

/// Format `fmt` against a win64 va_list *pointer* `va` (successive 8-byte
/// slots), producing the narrow-family (`printf`/`sprintf`) result.
///
/// SAFETY: `va` must point at the spilled argument array (guest VA == host VA);
/// scalar reads are 8 bytes each, string args are bounded (see [`format_va`]).
pub unsafe fn vformat_narrow(fmt: &[u8], va: u64) -> String {
    let mut p = va;
    let mut next = move || {
        let v = if p == 0 {
            0
        } else {
            core::ptr::read(p as *const u64)
        };
        p = p.wrapping_add(8);
        v
    };
    format_va(fmt, &mut next, false)
}

/// Wide-family (`wprintf`/`swprintf`) variant. The format *specifiers* are ASCII;
/// literal text is transcoded byte-wise (Latin-1) which is faithful for the
/// ASCII/Latin-1 literals real wide format strings use — the wide *arguments*
/// (`%ls`) are still read as full UTF-16.
///
/// SAFETY: as [`vformat_narrow`].
pub unsafe fn vformat_wide(fmt: &[u16], va: u64) -> String {
    let flen = fmt.iter().position(|&c| c == 0).unwrap_or(fmt.len());
    let mut bytes: Vec<u8> = fmt[..flen]
        .iter()
        .map(|&u| if u <= 0xFF { u as u8 } else { b'?' })
        .collect();
    bytes.push(0);
    let mut p = va;
    let mut next = move || {
        let v = if p == 0 {
            0
        } else {
            core::ptr::read(p as *const u64)
        };
        p = p.wrapping_add(8);
        v
    };
    format_va(&bytes, &mut next, true)
}

/// Copy a formatted `String` into a narrow output buffer with `count` capacity
/// (MSVC `_snprintf`/`_vsnprintf` contract): writes at most `count` bytes; if
/// the whole string + NUL fits, NUL-terminates and returns the length; on
/// truncation returns `-1` (and, per MSVC, may leave the buffer un-terminated).
pub fn write_narrow_into(buf: &mut [u8], count: usize, s: &str) -> i32 {
    let cap = count.min(buf.len());
    let src: Vec<u8> = s
        .chars()
        .map(|c| if (c as u32) <= 0xFF { c as u8 } else { b'?' })
        .collect();
    if cap == 0 {
        return -1;
    }
    if src.len() < cap {
        buf[..src.len()].copy_from_slice(&src);
        buf[src.len()] = 0;
        src.len() as i32
    } else {
        // Truncate to cap bytes (no guaranteed NUL — matches MSVC `_snprintf`).
        buf[..cap].copy_from_slice(&src[..cap]);
        -1
    }
}

/// Wide analogue of [`write_narrow_into`] (UTF-16 units).
pub fn write_wide_into(buf: &mut [u16], count: usize, s: &str) -> i32 {
    let cap = count.min(buf.len());
    let src: Vec<u16> = s.encode_utf16().collect();
    if cap == 0 {
        return -1;
    }
    if src.len() < cap {
        buf[..src.len()].copy_from_slice(&src);
        buf[src.len()] = 0;
        src.len() as i32
    } else {
        buf[..cap].copy_from_slice(&src[..cap]);
        -1
    }
}

/// C99 `snprintf` write semantics (the UCRT `__stdio_common_vsprintf` core):
/// write at most `cap-1` bytes then a NUL, but ALWAYS return the *full* length
/// the string would have needed. This lets `sprintf` (cap == huge) and the
/// bounded `snprintf`/`vsnprintf` wrappers share one path and lets a caller
/// size a buffer by passing a zero-length one.
pub fn write_narrow_c99(buf: &mut [u8], s: &str) -> i32 {
    let src: Vec<u8> = s
        .chars()
        .map(|c| if (c as u32) <= 0xFF { c as u8 } else { b'?' })
        .collect();
    let full = src.len();
    if !buf.is_empty() {
        let n = src.len().min(buf.len() - 1);
        buf[..n].copy_from_slice(&src[..n]);
        buf[n] = 0;
    }
    full as i32
}

/// Wide C99 `snwprintf` write semantics (UTF-16 units).
pub fn write_wide_c99(buf: &mut [u16], s: &str) -> i32 {
    let src: Vec<u16> = s.encode_utf16().collect();
    let full = src.len();
    if !buf.is_empty() {
        let n = src.len().min(buf.len() - 1);
        buf[..n].copy_from_slice(&src[..n]);
        buf[n] = 0;
    }
    full as i32
}

/// Read a NUL-terminated narrow format string at `ptr` into an owned byte vec
/// (trailing NUL guaranteed) for [`format_va`]. Bounded by [`FMT_MAX_STR`].
///
/// SAFETY: `ptr` is a live guest VA (guest VA == host VA) or 0.
pub unsafe fn read_format_bytes(ptr: u64) -> Vec<u8> {
    let mut v = Vec::new();
    if ptr == 0 {
        v.push(0);
        return v;
    }
    for i in 0..FMT_MAX_STR {
        let b = core::ptr::read((ptr + i as u64) as *const u8);
        v.push(b);
        if b == 0 {
            break;
        }
    }
    if v.last() != Some(&0) {
        v.push(0);
    }
    v
}

/// Read a NUL-terminated wide format string at `ptr` into an owned UTF-16 vec.
///
/// SAFETY: as [`read_format_bytes`].
pub unsafe fn read_format_wide(ptr: u64) -> Vec<u16> {
    let mut v = Vec::new();
    if ptr == 0 {
        v.push(0);
        return v;
    }
    for i in 0..FMT_MAX_STR {
        let u = core::ptr::read((ptr + (i * 2) as u64) as *const u16);
        v.push(u);
        if u == 0 {
            break;
        }
    }
    if v.last() != Some(&0) {
        v.push(0);
    }
    v
}

/// Format `fmt` against an in-memory argument slice, narrow family — the
/// String-producing core the variadic `sprintf`/`printf` shims build on.
pub fn vformat_args_narrow(fmt: &[u8], args: &[u64]) -> String {
    let mut i = 0usize;
    let mut next = || {
        let v = args.get(i).copied().unwrap_or(0);
        i += 1;
        v
    };
    format_va(fmt, &mut next, false)
}

/// Wide-family counterpart of [`vformat_args_narrow`].
pub fn vformat_args_wide(fmt: &[u16], args: &[u64]) -> String {
    let flen = fmt.iter().position(|&c| c == 0).unwrap_or(fmt.len());
    let mut bytes: Vec<u8> = fmt[..flen]
        .iter()
        .map(|&u| if u <= 0xFF { u as u8 } else { b'?' })
        .collect();
    bytes.push(0);
    let mut i = 0usize;
    let mut next = || {
        let v = args.get(i).copied().unwrap_or(0);
        i += 1;
        v
    };
    format_va(&bytes, &mut next, true)
}

// ---------------------------------------------------------------------------
// Security-enhanced functions
// ---------------------------------------------------------------------------

pub fn strcpy_s(dst: &mut [u8], src: &[u8]) -> i32 {
    if dst.is_empty() {
        _set_errno(EINVAL);
        return EINVAL;
    }
    let src_len = strlen(src);
    if src_len >= dst.len() {
        dst[0] = 0;
        _set_errno(ERANGE);
        return ERANGE;
    }
    dst[..src_len].copy_from_slice(&src[..src_len]);
    dst[src_len] = 0;
    0
}

pub fn strncpy_s(dst: &mut [u8], src: &[u8], count: usize) -> i32 {
    if dst.is_empty() {
        _set_errno(EINVAL);
        return EINVAL;
    }
    let src_len = strlen(src);
    let to_copy = src_len.min(count);
    if to_copy >= dst.len() {
        dst[0] = 0;
        _set_errno(ERANGE);
        return ERANGE;
    }
    dst[..to_copy].copy_from_slice(&src[..to_copy]);
    dst[to_copy] = 0;
    0
}

pub fn strcat_s(dst: &mut [u8], src: &[u8]) -> i32 {
    let dst_len = strlen(dst);
    let src_len = strlen(src);
    if dst_len + src_len >= dst.len() {
        dst[0] = 0;
        _set_errno(ERANGE);
        return ERANGE;
    }
    dst[dst_len..dst_len + src_len].copy_from_slice(&src[..src_len]);
    dst[dst_len + src_len] = 0;
    0
}

pub fn strncat_s(dst: &mut [u8], src: &[u8], count: usize) -> i32 {
    let dst_len = strlen(dst);
    let src_len = strlen(src);
    let to_copy = src_len.min(count);
    if dst_len + to_copy >= dst.len() {
        dst[0] = 0;
        _set_errno(ERANGE);
        return ERANGE;
    }
    dst[dst_len..dst_len + to_copy].copy_from_slice(&src[..to_copy]);
    dst[dst_len + to_copy] = 0;
    0
}

pub fn memcpy_s(dst: &mut [u8], dst_size: usize, src: &[u8], count: usize) -> i32 {
    if count > dst_size || count > dst.len() || count > src.len() {
        if !dst.is_empty() {
            memset(dst, 0, dst_size.min(dst.len()));
        }
        _set_errno(ERANGE);
        return ERANGE;
    }
    dst[..count].copy_from_slice(&src[..count]);
    0
}

pub fn memmove_s(dst: &mut [u8], dst_size: usize, src: &[u8], count: usize) -> i32 {
    if count > dst_size || count > dst.len() || count > src.len() {
        _set_errno(ERANGE);
        return ERANGE;
    }
    let tmp: Vec<u8> = src[..count].to_vec();
    dst[..count].copy_from_slice(&tmp);
    0
}

pub fn wcscpy_s(dst: &mut [u16], src: &[u16]) -> i32 {
    if dst.is_empty() {
        return EINVAL;
    }
    let src_len = wcslen(src);
    if src_len >= dst.len() {
        dst[0] = 0;
        return ERANGE;
    }
    dst[..src_len].copy_from_slice(&src[..src_len]);
    dst[src_len] = 0;
    0
}

pub fn wcsncpy_s(dst: &mut [u16], src: &[u16], count: usize) -> i32 {
    if dst.is_empty() {
        return EINVAL;
    }
    let src_len = wcslen(src);
    let to_copy = src_len.min(count);
    if to_copy >= dst.len() {
        dst[0] = 0;
        return ERANGE;
    }
    dst[..to_copy].copy_from_slice(&src[..to_copy]);
    dst[to_copy] = 0;
    0
}

// ---------------------------------------------------------------------------
// Time functions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
pub struct CrtTime {
    pub sec: i32,
    pub min: i32,
    pub hour: i32,
    pub mday: i32,
    pub mon: i32,
    pub year: i32,
    pub wday: i32,
    pub yday: i32,
    pub isdst: i32,
}

static TIME_BOOT_EPOCH: core::sync::atomic::AtomicI64 =
    core::sync::atomic::AtomicI64::new(1_700_000_000);
static TIME_CALL_COUNT: core::sync::atomic::AtomicI64 = core::sync::atomic::AtomicI64::new(0);

pub fn time() -> i64 {
    let base = TIME_BOOT_EPOCH.load(core::sync::atomic::Ordering::Relaxed);
    let n = TIME_CALL_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    base + n
}

pub fn set_boot_epoch(epoch_secs: i64) {
    TIME_BOOT_EPOCH.store(epoch_secs, core::sync::atomic::Ordering::Relaxed);
}

pub fn clock() -> i64 {
    0 // stub: return processor time
}

pub fn difftime(end: i64, start: i64) -> f64 {
    (end - start) as f64
}

pub fn mktime(tm: &CrtTime) -> i64 {
    let days_in_month: [i32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut days: i64 = 0;
    let year = tm.year + 1900;
    for y in 1970..year {
        days += if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
            366
        } else {
            365
        };
    }
    for m in 0..tm.mon {
        days += days_in_month[m as usize] as i64;
        if m == 1 && year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
            days += 1;
        }
    }
    days += (tm.mday - 1) as i64;
    days * 86400 + tm.hour as i64 * 3600 + tm.min as i64 * 60 + tm.sec as i64
}

pub fn gmtime(timestamp: i64) -> CrtTime {
    let mut t = timestamp;
    let sec = (t % 60) as i32;
    t /= 60;
    let min = (t % 60) as i32;
    t /= 60;
    let hour = (t % 24) as i32;
    t /= 24;

    let mut wday = ((t + 4) % 7) as i32; // 1970-01-01 was Thursday
    if wday < 0 {
        wday += 7;
    }

    let mut year: i32 = 1970;
    loop {
        let days_in_year: i64 = if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
            366
        } else {
            365
        };
        if t < days_in_year {
            break;
        }
        t -= days_in_year;
        year += 1;
    }

    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_month: [i64; 12] = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut mon: i32 = 0;
    for m in 0..12 {
        if t < days_in_month[m] {
            mon = m as i32;
            break;
        }
        t -= days_in_month[m];
    }

    CrtTime {
        sec,
        min,
        hour,
        mday: t as i32 + 1,
        mon,
        year: year - 1900,
        wday,
        yday: 0,
        isdst: 0,
    }
}

pub fn localtime(timestamp: i64) -> CrtTime {
    gmtime(timestamp) // stub: no timezone support
}

// ---------------------------------------------------------------------------
// Locale
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocaleCategory {
    All = 0,
    Collate = 1,
    CType = 2,
    Monetary = 3,
    Numeric = 4,
    Time = 5,
}

pub struct LocaleState {
    pub current_locale: String,
    pub multibyte_code_page: u32,
}

impl LocaleState {
    pub fn new() -> Self {
        Self {
            current_locale: String::from("C"),
            multibyte_code_page: 65001, // UTF-8
        }
    }

    pub fn setlocale(&mut self, _category: LocaleCategory, locale: &str) -> &str {
        if !locale.is_empty() {
            self.current_locale = String::from(locale);
        }
        // stub: always return current locale
        "C"
    }

    pub fn _setmbcp(&mut self, codepage: u32) {
        self.multibyte_code_page = codepage;
    }
    pub fn _getmbcp(&self) -> u32 {
        self.multibyte_code_page
    }
}

pub fn mbstowcs(dst: &mut [u16], src: &[u8], count: usize) -> usize {
    let src_len = strlen(src);
    let to_convert = src_len.min(count).min(dst.len());
    for i in 0..to_convert {
        dst[i] = src[i] as u16;
    }
    if to_convert < dst.len() {
        dst[to_convert] = 0;
    }
    to_convert
}

pub fn wcstombs(dst: &mut [u8], src: &[u16], count: usize) -> usize {
    let src_len = wcslen(src);
    let to_convert = src_len.min(count).min(dst.len());
    for i in 0..to_convert {
        dst[i] = if src[i] < 128 { src[i] as u8 } else { b'?' };
    }
    if to_convert < dst.len() {
        dst[to_convert] = 0;
    }
    to_convert
}

pub fn mbtowc(src: &[u8]) -> Option<(u16, usize)> {
    if src.is_empty() || src[0] == 0 {
        return None;
    }
    Some((src[0] as u16, 1))
}

pub fn wctomb(dst: &mut [u8], wc: u16) -> usize {
    if dst.is_empty() {
        return 0;
    }
    dst[0] = if wc < 128 { wc as u8 } else { b'?' };
    1
}

// ---------------------------------------------------------------------------
// Process functions
// ---------------------------------------------------------------------------

pub type ThreadFunc = fn(u64) -> u32;

#[derive(Debug, Clone)]
pub struct ThreadHandle {
    pub id: u64,
    pub active: bool,
}

pub struct ProcessState {
    pub threads: Vec<ThreadHandle>,
    pub next_thread_id: u64,
    pub exit_code: Option<i32>,
    pub atexit_handlers: Vec<fn()>,
    pub environment: Vec<(String, String)>,
}

impl ProcessState {
    pub fn new() -> Self {
        Self {
            threads: Vec::new(),
            next_thread_id: 1000,
            exit_code: None,
            atexit_handlers: Vec::new(),
            environment: Vec::new(),
        }
    }

    pub fn begin_thread(&mut self) -> u64 {
        let id = self.next_thread_id;
        self.next_thread_id += 1;
        self.threads.push(ThreadHandle { id, active: true });
        id
    }

    pub fn end_thread(&mut self, id: u64) {
        if let Some(t) = self.threads.iter_mut().find(|t| t.id == id) {
            t.active = false;
        }
    }

    pub fn exit(&mut self, code: i32) {
        for handler in self.atexit_handlers.iter().rev() {
            handler();
        }
        self.exit_code = Some(code);
    }

    pub fn abort(&mut self) {
        self.exit_code = Some(3);
    }

    pub fn atexit(&mut self, handler: fn()) {
        self.atexit_handlers.push(handler);
    }

    pub fn getenv(&self, name: &str) -> Option<&str> {
        self.environment
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn putenv(&mut self, name: &str, value: &str) {
        if let Some(entry) = self.environment.iter_mut().find(|(k, _)| k == name) {
            entry.1 = String::from(value);
        } else {
            self.environment
                .push((String::from(name), String::from(value)));
        }
    }
}

// ---------------------------------------------------------------------------
// Heap debugging (_Crt functions)
// ---------------------------------------------------------------------------

pub const _CRTDBG_ALLOC_MEM_DF: u32 = 0x01;
pub const _CRTDBG_DELAY_FREE_MEM_DF: u32 = 0x02;
pub const _CRTDBG_CHECK_ALWAYS_DF: u32 = 0x04;
pub const _CRTDBG_CHECK_CRT_DF: u32 = 0x10;
pub const _CRTDBG_LEAK_CHECK_DF: u32 = 0x20;

#[derive(Debug, Clone, Copy, Default)]
pub struct CrtMemState {
    pub blocks_allocated: u64,
    pub bytes_allocated: u64,
    pub high_water_mark: u64,
}

pub struct CrtDebug {
    pub flags: u32,
    pub report_mode: u32,
    pub break_alloc: Option<u64>,
    pub mem_state: CrtMemState,
}

impl CrtDebug {
    pub fn new() -> Self {
        Self {
            flags: _CRTDBG_ALLOC_MEM_DF,
            report_mode: 0,
            break_alloc: None,
            mem_state: CrtMemState::default(),
        }
    }

    pub fn set_dbg_flag(&mut self, flags: u32) -> u32 {
        let old = self.flags;
        self.flags = flags;
        old
    }

    pub fn set_report_mode(&mut self, mode: u32) -> u32 {
        let old = self.report_mode;
        self.report_mode = mode;
        old
    }

    pub fn check_memory(&self, heap: &CrtHeap) -> bool {
        heap.check_memory()
    }

    pub fn set_break_alloc(&mut self, alloc_id: u64) {
        self.break_alloc = Some(alloc_id);
    }

    pub fn dump_memory_leaks(&self, heap: &CrtHeap) -> Vec<(u64, usize)> {
        heap.dump_leaks()
    }

    pub fn checkpoint(&mut self, heap: &CrtHeap) {
        self.mem_state = CrtMemState {
            blocks_allocated: heap.alloc_count,
            bytes_allocated: heap.total_allocated as u64,
            high_water_mark: heap.next_addr,
        };
    }
}

// ---------------------------------------------------------------------------
// Global CRT_RUNTIME
// ---------------------------------------------------------------------------

pub struct CrtRuntime {
    pub initialized: AtomicBool,
    pub heap: Option<CrtHeap>,
    pub files: Vec<CrtFile>,
    pub next_file_id: u64,
    pub process: Option<ProcessState>,
    pub locale: Option<LocaleState>,
    pub debug: Option<CrtDebug>,
}

impl CrtRuntime {
    pub const fn new() -> Self {
        Self {
            initialized: AtomicBool::new(false),
            heap: None,
            files: Vec::new(),
            next_file_id: 3,
            process: None,
            locale: None,
            debug: None,
        }
    }

    pub fn init(&mut self) {
        if self.initialized.load(Ordering::Acquire) {
            return;
        }

        self.heap = Some(CrtHeap::new());
        self.process = Some(ProcessState::new());
        self.locale = Some(LocaleState::new());
        self.debug = Some(CrtDebug::new());

        // stdin(0), stdout(1), stderr(2)
        self.files
            .push(CrtFile::new(0, String::from("stdin"), FileMode::Read));
        self.files
            .push(CrtFile::new(1, String::from("stdout"), FileMode::Write));
        self.files
            .push(CrtFile::new(2, String::from("stderr"), FileMode::Write));

        self.initialized.store(true, Ordering::Release);
    }

    pub fn shutdown(&mut self) {
        if let Some(ref mut proc) = self.process {
            proc.exit(0);
        }
        self.heap = None;
        self.process = None;
        self.locale = None;
        self.debug = None;
        self.files.clear();
        self.initialized.store(false, Ordering::Release);
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }

    pub fn fopen(&mut self, path: &str, mode: FileMode) -> Option<u64> {
        let id = self.next_file_id;
        self.next_file_id += 1;
        self.files.push(CrtFile::new(id, String::from(path), mode));
        Some(id)
    }

    pub fn fclose(&mut self, id: u64) -> i32 {
        if let Some(pos) = self.files.iter().position(|f| f.id == id) {
            self.files.remove(pos);
            0
        } else {
            -1
        }
    }

    pub fn get_file(&mut self, id: u64) -> Option<&mut CrtFile> {
        self.files.iter_mut().find(|f| f.id == id)
    }

    pub fn heap(&mut self) -> Option<&mut CrtHeap> {
        self.heap.as_mut()
    }

    pub fn process(&mut self) -> Option<&mut ProcessState> {
        self.process.as_mut()
    }

    pub fn locale(&mut self) -> Option<&mut LocaleState> {
        self.locale.as_mut()
    }

    pub fn debug(&mut self) -> Option<&mut CrtDebug> {
        self.debug.as_mut()
    }
}

pub static mut CRT_RUNTIME: CrtRuntime = CrtRuntime::new();

pub fn init() {
    unsafe { CRT_RUNTIME.init() }
}

/// Lazily bring up the CRT heap + stdio tables on first allocator/file call.
fn crt_ensure_init() {
    unsafe {
        if !CRT_RUNTIME.is_initialized() {
            CRT_RUNTIME.init();
        }
    }
}

/// `malloc` — guest heap block from the per-process CRT allocator.
pub fn malloc(size: usize) -> u64 {
    crt_ensure_init();
    unsafe {
        CRT_RUNTIME
            .heap
            .as_mut()
            .map(|h| h.malloc(size))
            .unwrap_or(0)
    }
}

/// `calloc` — allocate `count * size` bytes (zero-fill deferred until guest
/// backing store is wired; the pointer contract matches Windows).
pub fn calloc(count: usize, size: usize) -> u64 {
    crt_ensure_init();
    unsafe {
        CRT_RUNTIME
            .heap
            .as_mut()
            .map(|h| h.calloc(count, size))
            .unwrap_or(0)
    }
}

/// `realloc` — resize or free+alloc when `size == 0`.
pub fn realloc(ptr: u64, size: usize) -> u64 {
    crt_ensure_init();
    unsafe {
        CRT_RUNTIME
            .heap
            .as_mut()
            .map(|h| h.realloc(ptr, size))
            .unwrap_or(0)
    }
}

/// `free` — release a CRT heap block.
pub fn free(ptr: u64) {
    crt_ensure_init();
    unsafe {
        if let Some(heap) = CRT_RUNTIME.heap.as_mut() {
            heap.free(ptr);
        }
    }
}

/// CRT per-FD lock — no-op until multi-threaded stdio is modeled.
pub fn _lock(_fd: i32) {}

/// CRT per-FD unlock — paired with [`_lock`].
pub fn _unlock(_fd: i32) {}

// ---------------------------------------------------------------------------
// Low-level file I/O (_open, _close, _read, _write, _lseek)
// ---------------------------------------------------------------------------

pub fn _open(path: &[u8], flags: i32, _mode: i32) -> i32 {
    let runtime = unsafe { &mut CRT_RUNTIME };
    if !runtime.is_initialized() {
        return -1;
    }
    let name_len = path.iter().position(|&b| b == 0).unwrap_or(path.len());
    let name = core::str::from_utf8(&path[..name_len]).unwrap_or("unknown");
    let file_mode = if flags & 0x0001 != 0 {
        FileMode::Write
    } else if flags & 0x0002 != 0 {
        FileMode::ReadWrite
    } else {
        FileMode::Read
    };
    match runtime.fopen(name, file_mode) {
        Some(id) => id as i32,
        None => -1,
    }
}

pub fn _wopen(path: &[u16], flags: i32, mode: i32) -> i32 {
    let mut buf = [0u8; 260];
    let len = path.iter().take_while(|&&c| c != 0).count();
    let copy_len = core::cmp::min(len, buf.len() - 1);
    for i in 0..copy_len {
        buf[i] = if path[i] <= 0xFF { path[i] as u8 } else { b'?' };
    }
    buf[copy_len] = 0;
    _open(&buf, flags, mode)
}

pub fn _close(fd: i32) -> i32 {
    let runtime = unsafe { &mut CRT_RUNTIME };
    runtime.fclose(fd as u64)
}

pub fn _read(fd: i32, buf: &mut [u8], count: u32) -> i32 {
    let runtime = unsafe { &mut CRT_RUNTIME };
    if let Some(file) = runtime.get_file(fd as u64) {
        let avail = file.data.len() - file.position;
        let to_read = core::cmp::min(count as usize, avail);
        if to_read > 0 {
            let to_read_bounded = core::cmp::min(to_read, buf.len());
            buf[..to_read_bounded]
                .copy_from_slice(&file.data[file.position..file.position + to_read_bounded]);
            file.position += to_read_bounded;
            to_read_bounded as i32
        } else {
            0
        }
    } else {
        -1
    }
}

pub fn _write(fd: i32, buf: &[u8], count: u32) -> i32 {
    let runtime = unsafe { &mut CRT_RUNTIME };
    if let Some(file) = runtime.get_file(fd as u64) {
        let to_write = core::cmp::min(count as usize, buf.len());
        file.data.extend_from_slice(&buf[..to_write]);
        file.position = file.data.len();
        to_write as i32
    } else {
        -1
    }
}

pub fn _lseek(fd: i32, offset: i64, origin: i32) -> i64 {
    let runtime = unsafe { &mut CRT_RUNTIME };
    if let Some(file) = runtime.get_file(fd as u64) {
        let new_pos = match origin {
            0 => offset as usize,                            // SEEK_SET
            1 => (file.position as i64 + offset) as usize,   // SEEK_CUR
            2 => (file.data.len() as i64 + offset) as usize, // SEEK_END
            _ => return -1,
        };
        file.position = new_pos;
        new_pos as i64
    } else {
        -1
    }
}

pub fn _lseeki64(fd: i32, offset: i64, origin: i32) -> i64 {
    _lseek(fd, offset, origin)
}

pub fn _filelength(fd: i32) -> i64 {
    let runtime = unsafe { &mut CRT_RUNTIME };
    if let Some(file) = runtime.get_file(fd as u64) {
        file.data.len() as i64
    } else {
        -1
    }
}

pub fn _isatty(fd: i32) -> i32 {
    if fd <= 2 {
        1
    } else {
        0
    }
}

pub fn _fileno(_stream: u64) -> i32 {
    -1
}

// ---------------------------------------------------------------------------
// Wide file I/O
// ---------------------------------------------------------------------------

pub fn _wfopen(path: &[u16], mode: &[u16]) -> i64 {
    let runtime = unsafe { &mut CRT_RUNTIME };
    if !runtime.is_initialized() {
        return 0;
    }
    let name_len = path.iter().take_while(|&&c| c != 0).count();
    let mut name = String::new();
    for &ch in &path[..name_len] {
        name.push(if ch < 128 { ch as u8 as char } else { '?' });
    }
    let first_mode = if !mode.is_empty() {
        mode[0]
    } else {
        b'r' as u16
    };
    let fm = match first_mode {
        119 => FileMode::Write, // 'w'
        97 => FileMode::Append, // 'a'
        _ => FileMode::Read,
    };
    match runtime.fopen(&name, fm) {
        Some(id) => id as i64,
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// Threading (_beginthreadex, _endthreadex)
// ---------------------------------------------------------------------------

pub fn _beginthreadex(
    _security: u64,
    _stack_size: u32,
    _start_address: u64,
    _arglist: u64,
    _initflag: u32,
    thread_id: &mut u32,
) -> u64 {
    let runtime = unsafe { &mut CRT_RUNTIME };
    if let Some(ref mut proc) = runtime.process {
        let id = proc.begin_thread();
        *thread_id = id as u32;
        id
    } else {
        0
    }
}

pub fn _endthreadex(_retval: u32) {
    // thread cleanup — in emulation, no-op
}

pub fn _beginthread(_start_address: u64, _stack_size: u32, _arglist: u64) -> u64 {
    let runtime = unsafe { &mut CRT_RUNTIME };
    if let Some(ref mut proc) = runtime.process {
        proc.begin_thread()
    } else {
        u64::MAX
    }
}

pub fn _endthread() {
    // no-op
}

// ---------------------------------------------------------------------------
// String functions (case-insensitive, wide-to-narrow, etc.)
// ---------------------------------------------------------------------------

pub fn _stricmp(s1: &[u8], s2: &[u8]) -> i32 {
    let len = core::cmp::min(
        s1.iter().position(|&b| b == 0).unwrap_or(s1.len()),
        s2.iter().position(|&b| b == 0).unwrap_or(s2.len()),
    );
    for i in 0..len {
        let a = if s1[i] >= b'A' && s1[i] <= b'Z' {
            s1[i] + 32
        } else {
            s1[i]
        };
        let b = if s2[i] >= b'A' && s2[i] <= b'Z' {
            s2[i] + 32
        } else {
            s2[i]
        };
        if a != b {
            return (a as i32) - (b as i32);
        }
    }
    let l1 = s1.iter().position(|&b| b == 0).unwrap_or(s1.len());
    let l2 = s2.iter().position(|&b| b == 0).unwrap_or(s2.len());
    (l1 as i32) - (l2 as i32)
}

pub fn _strnicmp(s1: &[u8], s2: &[u8], n: usize) -> i32 {
    let l1 = s1.iter().position(|&b| b == 0).unwrap_or(s1.len());
    let l2 = s2.iter().position(|&b| b == 0).unwrap_or(s2.len());
    let len = core::cmp::min(core::cmp::min(l1, l2), n);
    for i in 0..len {
        let a = if s1[i] >= b'A' && s1[i] <= b'Z' {
            s1[i] + 32
        } else {
            s1[i]
        };
        let b = if s2[i] >= b'A' && s2[i] <= b'Z' {
            s2[i] + 32
        } else {
            s2[i]
        };
        if a != b {
            return (a as i32) - (b as i32);
        }
    }
    0
}

pub fn _wcsicmp(s1: &[u16], s2: &[u16]) -> i32 {
    let l1 = s1.iter().position(|&c| c == 0).unwrap_or(s1.len());
    let l2 = s2.iter().position(|&c| c == 0).unwrap_or(s2.len());
    let len = core::cmp::min(l1, l2);
    for i in 0..len {
        let a = if s1[i] >= b'A' as u16 && s1[i] <= b'Z' as u16 {
            s1[i] + 32
        } else {
            s1[i]
        };
        let b = if s2[i] >= b'A' as u16 && s2[i] <= b'Z' as u16 {
            s2[i] + 32
        } else {
            s2[i]
        };
        if a != b {
            return (a as i32) - (b as i32);
        }
    }
    (l1 as i32) - (l2 as i32)
}

pub fn _wcsnicmp(s1: &[u16], s2: &[u16], n: usize) -> i32 {
    let l1 = s1.iter().position(|&c| c == 0).unwrap_or(s1.len());
    let l2 = s2.iter().position(|&c| c == 0).unwrap_or(s2.len());
    let len = core::cmp::min(core::cmp::min(l1, l2), n);
    for i in 0..len {
        let a = if s1[i] >= b'A' as u16 && s1[i] <= b'Z' as u16 {
            s1[i] + 32
        } else {
            s1[i]
        };
        let b = if s2[i] >= b'A' as u16 && s2[i] <= b'Z' as u16 {
            s2[i] + 32
        } else {
            s2[i]
        };
        if a != b {
            return (a as i32) - (b as i32);
        }
    }
    0
}

pub fn _wtoi(s: &[u16]) -> i32 {
    let len = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    let mut buf = [0u8; 32];
    let copy_len = core::cmp::min(len, buf.len());
    for i in 0..copy_len {
        buf[i] = if s[i] <= 0xFF { s[i] as u8 } else { 0 };
    }
    atoi(&buf[..copy_len])
}

pub fn _wtol(s: &[u16]) -> i64 {
    let len = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    let mut buf = [0u8; 32];
    let copy_len = core::cmp::min(len, buf.len());
    for i in 0..copy_len {
        buf[i] = if s[i] <= 0xFF { s[i] as u8 } else { 0 };
    }
    atol(&buf[..copy_len])
}

pub fn _itow(value: i32, buf: &mut [u16], radix: u32) -> usize {
    let mut tmp = [0u8; 34];
    let len = itoa(value, &mut tmp, radix);
    let copy_len = core::cmp::min(len, buf.len().saturating_sub(1));
    for i in 0..copy_len {
        buf[i] = tmp[i] as u16;
    }
    if copy_len < buf.len() {
        buf[copy_len] = 0;
    }
    copy_len
}

/// `_snprintf(buf, count, fmt, args)` — real formatter driven by an in-memory
/// argument slice (host-KAT entry point: scalar args need no guest pointers).
pub fn _snprintf(buf: &mut [u8], count: usize, fmt: &[u8], args: &[u64]) -> i32 {
    let mut idx = 0usize;
    let mut next = || {
        let v = args.get(idx).copied().unwrap_or(0);
        idx += 1;
        v
    };
    let s = format_va(fmt, &mut next, false);
    write_narrow_into(buf, count, &s)
}

/// Wide `_snwprintf` counterpart. The format string is ASCII-transcoded (see
/// [`vformat_wide`]); `args` supplies the scalar/pointer arguments.
pub fn _snwprintf(buf: &mut [u16], count: usize, fmt: &[u16], args: &[u64]) -> i32 {
    let flen = fmt.iter().position(|&c| c == 0).unwrap_or(fmt.len());
    let mut bytes: Vec<u8> = fmt[..flen]
        .iter()
        .map(|&u| if u <= 0xFF { u as u8 } else { b'?' })
        .collect();
    bytes.push(0);
    let mut idx = 0usize;
    let mut next = || {
        let v = args.get(idx).copied().unwrap_or(0);
        idx += 1;
        v
    };
    let s = format_va(&bytes, &mut next, true);
    write_wide_into(buf, count, &s)
}

/// `_vsnprintf(buf, count, fmt, va)` — the va_list-pointer entry point the
/// stdio shims call at runtime.
///
/// SAFETY: `va` is a live guest va_list pointer (guest VA == host VA).
pub unsafe fn _vsnprintf(buf: &mut [u8], count: usize, fmt: &[u8], va: u64) -> i32 {
    let s = vformat_narrow(fmt, va);
    write_narrow_into(buf, count, &s)
}

/// SAFETY: as [`_vsnprintf`].
pub unsafe fn _vsnwprintf(buf: &mut [u16], count: usize, fmt: &[u16], va: u64) -> i32 {
    let s = vformat_wide(fmt, va);
    write_wide_into(buf, count, &s)
}

// ---------------------------------------------------------------------------
// Signal
// ---------------------------------------------------------------------------

pub const SIGINT: i32 = 2;
pub const SIGILL: i32 = 4;
pub const SIGFPE: i32 = 8;
pub const SIGSEGV: i32 = 11;
pub const SIGTERM: i32 = 15;
pub const SIGABRT: i32 = 22;

static SIGNAL_HANDLERS: [core::sync::atomic::AtomicU64; 32] = {
    const INIT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    [INIT; 32]
};

pub fn signal(signum: i32, handler: u64) -> u64 {
    if signum < 0 || signum >= 32 {
        return u64::MAX;
    }
    SIGNAL_HANDLERS[signum as usize].swap(handler, core::sync::atomic::Ordering::Relaxed)
}

pub fn raise(signum: i32) -> i32 {
    if signum < 0 || signum >= 32 {
        return -1;
    }
    let _handler = SIGNAL_HANDLERS[signum as usize].load(core::sync::atomic::Ordering::Relaxed);
    0
}

// ---------------------------------------------------------------------------
// CRT init hooks (_initterm, _initterm_e)
// ---------------------------------------------------------------------------

pub fn _initterm(start: &[u64], _end: u64) {
    let _ = start;
}

pub fn _initterm_e(start: &[u64], _end: u64) -> i32 {
    let _ = start;
    0
}

// ---------------------------------------------------------------------------
// UCRT onexit / atexit registry (guest function pointers)
// ---------------------------------------------------------------------------
//
// The MSVC `/MD` startup registers C++ static destructors and user `atexit`
// callbacks through the ucrtbase onexit table, then runs them in LIFO order at
// `exit`/`_cexit`. `ProcessState::atexit` above stores *Rust* `fn()`s (host
// bookkeeping); the guest's callbacks are raw VAs, so they need their own
// registry. This is that registry — a serialized list of guest function
// pointers. The *bookkeeping* (register / LIFO drain / count) is pure and
// host-KAT'd; the actual `call` of each VA happens only in the on-target shim
// (guest VA == host VA), never in a host test.

struct OnexitRegistry {
    lock: AtomicBool,
    fns: core::cell::UnsafeCell<Vec<u64>>,
}

// SAFETY: every access serializes on `lock`.
unsafe impl Sync for OnexitRegistry {}

static ONEXIT: OnexitRegistry = OnexitRegistry {
    lock: AtomicBool::new(false),
    fns: core::cell::UnsafeCell::new(Vec::new()),
};

fn onexit_lock() {
    while ONEXIT
        .lock
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
}

fn onexit_unlock() {
    ONEXIT.lock.store(false, Ordering::Release);
}

/// Register a guest exit callback (`atexit`/`_onexit`/`_register_onexit_function`
/// /`_crt_atexit`). A null pointer is ignored. Returns `true` on success.
pub fn onexit_register(func: u64) -> bool {
    if func == 0 {
        return false;
    }
    onexit_lock();
    // SAFETY: lock held.
    unsafe {
        (*ONEXIT.fns.get()).push(func);
    }
    onexit_unlock();
    true
}

/// Drain the registry, returning the callbacks in LIFO order (the CRT runs the
/// most-recently-registered destructor first). Leaves the registry empty so a
/// second `_cexit` does not double-run destructors.
pub fn onexit_take_all() -> Vec<u64> {
    onexit_lock();
    // SAFETY: lock held.
    let mut v = unsafe { core::mem::take(&mut *ONEXIT.fns.get()) };
    onexit_unlock();
    v.reverse();
    v
}

/// Number of registered callbacks (smoketest/KAT visibility).
pub fn onexit_count() -> usize {
    onexit_lock();
    // SAFETY: lock held.
    let n = unsafe { (*ONEXIT.fns.get()).len() };
    onexit_unlock();
    n
}

#[cfg(test)]
pub fn onexit_clear_for_test() {
    onexit_lock();
    // SAFETY: lock held.
    unsafe {
        (*ONEXIT.fns.get()).clear();
    }
    onexit_unlock();
}

// ---------------------------------------------------------------------------
// Misc CRT
// ---------------------------------------------------------------------------

pub fn _set_invalid_parameter_handler(_handler: u64) -> u64 {
    0
}

pub fn _set_purecall_handler(_handler: u64) -> u64 {
    0
}

pub fn _set_new_handler(_handler: u64) -> u64 {
    0
}

pub fn _set_new_mode(mode: i32) -> i32 {
    let _ = mode;
    0
}

pub fn _get_osfhandle(fd: i32) -> i64 {
    fd as i64
}

pub fn _open_osfhandle(osfhandle: i64, _flags: i32) -> i32 {
    osfhandle as i32
}

pub fn _control87(_new: u32, _mask: u32) -> u32 {
    0x0001_001F // default FPU control word
}

pub fn _controlfp(_new: u32, _mask: u32) -> u32 {
    0x0001_001F
}

pub fn __getmainargs(
    argc: &mut i32,
    _argv: &mut u64,
    _envp: &mut u64,
    _do_wildcard: i32,
    _start_info: u64,
) -> i32 {
    *argc = 1;
    0
}

pub fn __wgetmainargs(
    argc: &mut i32,
    _argv: &mut u64,
    _envp: &mut u64,
    _do_wildcard: i32,
    _start_info: u64,
) -> i32 {
    *argc = 1;
    0
}

pub fn _crt_atexit(handler: fn()) -> i32 {
    let runtime = unsafe { &mut CRT_RUNTIME };
    if let Some(ref mut proc) = runtime.process {
        proc.atexit(handler);
    }
    0
}

pub fn _cexit() {
    let runtime = unsafe { &mut CRT_RUNTIME };
    runtime.shutdown();
}

pub fn _c_exit() {
    // fast exit, no cleanup
}

pub fn _errno() -> *mut i32 {
    static mut ERRNO_VAL: i32 = 0;
    unsafe { &mut ERRNO_VAL as *mut i32 }
}

pub fn _get_current_locale() -> u64 {
    0 // null locale handle
}

pub fn _create_locale(_category: i32, _locale: &[u8]) -> u64 {
    0
}

pub fn _free_locale(_locale: u64) {
    // no-op
}

pub fn _configure_narrow_argv(_mode: i32) -> i32 {
    0
}

pub fn _configure_wide_argv(_mode: i32) -> i32 {
    0
}

pub fn _initialize_narrow_environment() -> i32 {
    0
}

pub fn _initialize_wide_environment() -> i32 {
    0
}

pub fn _get_initial_narrow_environment() -> u64 {
    0
}

pub fn _get_initial_wide_environment() -> u64 {
    0
}

pub fn __p___argc() -> *mut i32 {
    static mut ARGC: i32 = 1;
    unsafe { &mut ARGC as *mut i32 }
}

pub fn __p___argv() -> u64 {
    0
}

pub fn __p___wargv() -> u64 {
    0
}

// ---------------------------------------------------------------------------
// Host KATs — the vararg format engine, proven off-target and FAIL-able
// ---------------------------------------------------------------------------

#[cfg(test)]
mod format_tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    /// Format a `&str` format + an in-memory arg slice through the real engine
    /// (narrow family). Mirrors the on-target `vformat_narrow` path but with a
    /// slice `va_arg` so the KAT needs no live guest pointers for scalars.
    fn f(fmt: &str, args: &[u64]) -> String {
        let mut b = fmt.as_bytes().to_vec();
        b.push(0);
        let mut i = 0usize;
        let mut next = || {
            let v = args.get(i).copied().unwrap_or(0);
            i += 1;
            v
        };
        format_va(&b, &mut next, false)
    }

    #[test]
    fn plain_text_and_escaped_percent() {
        assert_eq!(f("hello", &[]), "hello");
        assert_eq!(f("100%%", &[]), "100%");
        assert_eq!(f("", &[]), "");
    }

    #[test]
    fn signed_decimal_flags_width_precision() {
        assert_eq!(f("%d", &[42]), "42");
        // 32-bit sign extension: 0xFFFFFFFF == -1.
        assert_eq!(f("%d", &[0xFFFF_FFFF]), "-1");
        assert_eq!(f("%i", &[0]), "0");
        assert_eq!(f("%+d", &[42]), "+42");
        assert_eq!(f("% d", &[42]), " 42");
        assert_eq!(f("%5d", &[42]), "   42");
        assert_eq!(f("%-5d|", &[42]), "42   |");
        assert_eq!(f("%05d", &[42]), "00042");
        assert_eq!(f("%.4d", &[42]), "0042");
        // Precision 0 with value 0 emits no digits.
        assert_eq!(f("[%.0d]", &[0]), "[]");
        // 64-bit.
        assert_eq!(f("%lld", &[0x1_0000_0000]), "4294967296");
        assert_eq!(f("%I64d", &[u64::MAX]), "-1");
    }

    #[test]
    fn unsigned_hex_octal_and_hash() {
        assert_eq!(f("%u", &[0xFFFF_FFFF]), "4294967295");
        assert_eq!(f("%x", &[0xDEAD_BEEF]), "deadbeef");
        assert_eq!(f("%X", &[0xDEAD_BEEF]), "DEADBEEF");
        assert_eq!(f("%#x", &[0x2A]), "0x2a");
        assert_eq!(f("%#X", &[0x2A]), "0X2A");
        assert_eq!(f("%o", &[8]), "10");
        assert_eq!(f("%#o", &[8]), "010");
        assert_eq!(f("%08x", &[0x1234]), "00001234");
        // Hash on zero adds no prefix.
        assert_eq!(f("%#x", &[0]), "0");
    }

    #[test]
    fn char_and_literal_mix() {
        assert_eq!(f("%c%c%c", &[b'a' as u64, b'b' as u64, b'c' as u64]), "abc");
        assert_eq!(f("[%3c]", &[b'z' as u64]), "[  z]");
        assert_eq!(f("v=%d end", &[7]), "v=7 end");
    }

    #[test]
    fn narrow_string_arg_with_precision_and_width() {
        let s = b"AthBridge\0";
        let p = s.as_ptr() as u64;
        assert_eq!(f("%s", &[p]), "AthBridge");
        // Precision truncates to N chars.
        assert_eq!(f("%.3s", &[p]), "Rae");
        // Width right-justifies.
        assert_eq!(f("%12s", &[p]), "   AthBridge");
        assert_eq!(f("%-12s|", &[p]), "AthBridge   |");
        // NULL pointer → "(null)".
        assert_eq!(f("%s", &[0]), "(null)");
    }

    #[test]
    fn wide_string_arg_via_ls() {
        let w: Vec<u16> = "wíde\0".encode_utf16().collect();
        let p = w.as_ptr() as u64;
        assert_eq!(f("%ls", &[p]), "wíde");
    }

    #[test]
    fn pointer_is_16_upper_hex() {
        assert_eq!(f("%p", &[0xABCDEF]), "0000000000ABCDEF");
        assert_eq!(f("%p", &[0]), "0000000000000000");
    }

    #[test]
    fn float_fixed_scientific_general() {
        assert_eq!(f("%f", &[3.5f64.to_bits()]), "3.500000");
        assert_eq!(f("%.2f", &[3.14159f64.to_bits()]), "3.14");
        assert_eq!(f("%.0f", &[2.5f64.to_bits()]), "2");
        assert_eq!(f("%+.1f", &[1.25f64.to_bits()]), "+1.2");
        assert_eq!(f("%f", &[(-0.5f64).to_bits()]), "-0.500000");
        // Scientific: two-digit exponent, forced sign.
        assert_eq!(f("%.2e", &[12345.0f64.to_bits()]), "1.23e+04");
        assert_eq!(f("%.1e", &[0.0f64.to_bits()]), "0.0e+00");
        // General trims trailing zeros.
        assert_eq!(f("%g", &[100.0f64.to_bits()]), "100");
        assert_eq!(f("%g", &[0.0001f64.to_bits()]), "0.0001");
        // Non-finite.
        assert_eq!(f("%f", &[f64::INFINITY.to_bits()]), "inf");
        assert_eq!(f("%F", &[f64::NAN.to_bits()]), "NAN");
    }

    #[test]
    fn star_width_and_precision_consume_args() {
        // width=8, value=42.
        assert_eq!(f("%*d", &[8, 42]), "      42");
        // precision=3, value from string pointer.
        let s = b"abcdef\0";
        assert_eq!(f("%.*s", &[3, s.as_ptr() as u64]), "abc");
        // Negative star width → left-justify.
        assert_eq!(f("%*d|", &[(-5i64) as u64, 7]), "7    |");
    }

    #[test]
    fn multi_arg_ordering_is_left_to_right() {
        let name = b"Rae\0";
        let out = f(
            "%s=%d,%#x,%c",
            &[name.as_ptr() as u64, 10, 255, b'!' as u64],
        );
        assert_eq!(out, "Rae=10,0xff,!");
    }

    #[test]
    fn snprintf_truncation_contract() {
        // Fits: NUL-terminated, returns length.
        let mut buf = [0u8; 16];
        let n = _snprintf(&mut buf, 16, b"ab%d\0", &[7]);
        assert_eq!(n, 3);
        assert_eq!(&buf[..4], b"ab7\0");
        // Truncated: returns -1.
        let mut small = [0u8; 3];
        let n2 = _snprintf(&mut small, 3, b"abcdef\0", &[]);
        assert_eq!(n2, -1);
        assert_eq!(&small, b"abc");
    }

    #[test]
    fn unknown_conversion_is_emitted_verbatim() {
        // `%y` is not a real spec — engine passes it through, not a panic.
        assert_eq!(f("a%yb", &[]), "a%yb");
    }

    #[test]
    fn onexit_registry_is_lifo_and_drains_once() {
        onexit_clear_for_test();
        assert_eq!(onexit_count(), 0);
        // NULL is ignored.
        assert!(!onexit_register(0));
        assert_eq!(onexit_count(), 0);
        // Registration order 1,2,3 → LIFO drain 3,2,1 (CRT destructor order).
        assert!(onexit_register(0x1111));
        assert!(onexit_register(0x2222));
        assert!(onexit_register(0x3333));
        assert_eq!(onexit_count(), 3);
        assert_eq!(onexit_take_all(), alloc::vec![0x3333, 0x2222, 0x1111]);
        // Draining empties the registry (no double-run on a second _cexit).
        assert_eq!(onexit_count(), 0);
        assert!(onexit_take_all().is_empty());
        onexit_clear_for_test();
    }
}
