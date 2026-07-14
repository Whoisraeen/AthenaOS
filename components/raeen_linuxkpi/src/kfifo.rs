//! `kfifo` — lockless single-producer/single-consumer ring buffer.
//!
//! Drivers use kfifo for command/event queues (DRM GPU-scheduler entity FIFOs,
//! interrupt event rings, trace buffers). The user-facing `kfifo_in`/`kfifo_out`
//! are C macros that expand onto the exported `__kfifo_*` helpers — those carry
//! the real ring logic (power-of-two masking, wraparound copy, element sizing),
//! so the shim implements them faithfully. A stub that dropped elements or
//! mis-wrapped would silently corrupt a driver's event stream.
//!
//! `struct __kfifo` is LP64 `{ u32 in; u32 out; u32 mask; u32 esize; void *data; }`
//! (offsets 0/4/8/12/16). `in`/`out` are free-running element counters masked on
//! access; the count is `in - out` under wrapping u32 arithmetic. All lengths
//! are in ELEMENTS; bytes = elements * `esize`. Backing storage comes from the
//! daemon heap (`mm`).

use crate::mm;
use core::ptr;

/// `struct __kfifo` — must match the Linux LP64 layout (drivers embed it).
#[repr(C)]
pub struct Kfifo {
    pub r#in: u32,
    pub out: u32,
    pub mask: u32,
    pub esize: u32,
    pub data: *mut u8,
}

/// Smallest power of two ≥ `x` (Linux `roundup_pow_of_two`), min 1.
fn roundup_pow2(x: u32) -> u32 {
    if x <= 1 {
        return 1;
    }
    let mut p = 1u32;
    while p < x {
        match p.checked_mul(2) {
            Some(n) => p = n,
            None => return p, // saturate at the top power of two
        }
    }
    p
}

/// Copy `len` elements into the ring at logical offset `off` (= `in & mask`),
/// wrapping at the end of the backing store.
unsafe fn ring_copy_in(fifo: *mut Kfifo, src: *const u8, len: u32, off: u32) {
    let size = (*fifo).mask + 1;
    let esize = (*fifo).esize as usize;
    let data = (*fifo).data;
    let first = len.min(size - off); // elements until the wrap point
                                     // usize byte math (promote before multiply) avoids u32 overflow on big rings.
    ptr::copy_nonoverlapping(src, data.add(off as usize * esize), first as usize * esize);
    if len > first {
        ptr::copy_nonoverlapping(
            src.add(first as usize * esize),
            data,
            (len - first) as usize * esize,
        );
    }
}

/// Copy `len` elements out of the ring at logical offset `off` (= `out & mask`).
unsafe fn ring_copy_out(fifo: *mut Kfifo, dst: *mut u8, len: u32, off: u32) {
    let size = (*fifo).mask + 1;
    let esize = (*fifo).esize as usize;
    let data = (*fifo).data;
    let first = len.min(size - off); // elements until the wrap point
    ptr::copy_nonoverlapping(data.add(off as usize * esize), dst, first as usize * esize);
    if len > first {
        ptr::copy_nonoverlapping(
            data,
            dst.add(first as usize * esize),
            (len - first) as usize * esize,
        );
    }
}

/// `__kfifo_init(fifo, buffer, size, esize)` — wrap a caller-provided buffer.
/// `size` must be a power of two (number of elements). Returns 0 or `-EINVAL`.
#[no_mangle]
pub extern "C" fn __kfifo_init(fifo: *mut Kfifo, buffer: *mut u8, size: u32, esize: usize) -> i32 {
    if fifo.is_null() || size < 2 || (size & (size - 1)) != 0 {
        return -22; // -EINVAL
    }
    unsafe {
        (*fifo).r#in = 0;
        (*fifo).out = 0;
        (*fifo).esize = esize as u32;
        (*fifo).mask = size - 1;
        (*fifo).data = buffer;
    }
    0
}

/// `__kfifo_alloc(fifo, size, esize, gfp)` — allocate backing storage; `size`
/// (elements) is rounded up to a power of two. Returns 0, `-EINVAL`, or `-ENOMEM`.
#[no_mangle]
pub extern "C" fn __kfifo_alloc(fifo: *mut Kfifo, size: u32, esize: usize, _gfp: u32) -> i32 {
    if fifo.is_null() || esize == 0 {
        return -22;
    }
    let size = roundup_pow2(size);
    if size < 2 {
        return -22;
    }
    let bytes = (size as usize).saturating_mul(esize);
    let data = mm::kmalloc(bytes, 0);
    if data.is_null() {
        return -12; // -ENOMEM
    }
    unsafe {
        (*fifo).r#in = 0;
        (*fifo).out = 0;
        (*fifo).esize = esize as u32;
        (*fifo).mask = size - 1;
        (*fifo).data = data;
    }
    0
}

/// `__kfifo_free(fifo)`.
#[no_mangle]
pub extern "C" fn __kfifo_free(fifo: *mut Kfifo) {
    if fifo.is_null() {
        return;
    }
    unsafe {
        if !(*fifo).data.is_null() {
            mm::kfree((*fifo).data);
        }
        (*fifo).r#in = 0;
        (*fifo).out = 0;
        (*fifo).mask = 0;
        (*fifo).data = ptr::null_mut();
    }
}

#[inline]
unsafe fn used(fifo: *mut Kfifo) -> u32 {
    (*fifo).r#in.wrapping_sub((*fifo).out)
}

/// `__kfifo_in(fifo, buf, len)` — enqueue up to `len` elements; returns the
/// number actually written (limited by free space).
#[no_mangle]
pub extern "C" fn __kfifo_in(fifo: *mut Kfifo, buf: *const u8, len: u32) -> u32 {
    if fifo.is_null() || buf.is_null() {
        return 0;
    }
    unsafe {
        let size = (*fifo).mask + 1;
        let unused = size - used(fifo);
        let n = len.min(unused);
        if n > 0 {
            let off = (*fifo).r#in & (*fifo).mask;
            ring_copy_in(fifo, buf, n, off);
            (*fifo).r#in = (*fifo).r#in.wrapping_add(n);
        }
        n
    }
}

/// `__kfifo_out_peek(fifo, buf, len)` — copy up to `len` elements WITHOUT
/// consuming them; returns the number copied.
#[no_mangle]
pub extern "C" fn __kfifo_out_peek(fifo: *mut Kfifo, buf: *mut u8, len: u32) -> u32 {
    if fifo.is_null() || buf.is_null() {
        return 0;
    }
    unsafe {
        let n = len.min(used(fifo));
        if n > 0 {
            let off = (*fifo).out & (*fifo).mask;
            ring_copy_out(fifo, buf, n, off);
        }
        n
    }
}

/// `__kfifo_out(fifo, buf, len)` — dequeue up to `len` elements; returns the
/// number actually read.
#[no_mangle]
pub extern "C" fn __kfifo_out(fifo: *mut Kfifo, buf: *mut u8, len: u32) -> u32 {
    let n = __kfifo_out_peek(fifo, buf, len);
    if n > 0 && !fifo.is_null() {
        unsafe { (*fifo).out = (*fifo).out.wrapping_add(n) };
    }
    n
}

/// `__kfifo_len` helper (the `kfifo_len` macro reads `in - out` directly, but a
/// few call sites use the symbol): number of queued elements.
#[no_mangle]
pub extern "C" fn __kfifo_len(fifo: *mut Kfifo) -> u32 {
    if fifo.is_null() {
        return 0;
    }
    unsafe { used(fifo) }
}
