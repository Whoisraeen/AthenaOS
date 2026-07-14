//! `s*printf` family — bounded string formatting onto the shared interpolation
//! engine (`device::format_into`).
//!
//! Drivers format constantly: `snprintf` for sysfs/debug strings, `scnprintf`
//! for ring-buffer log builders, `kasprintf` to mint device/IRQ names. These all
//! share the `%`-specifier engine that `printk`/`dev_*` already use, so the
//! formatting is one tested implementation; these entries differ only in WHERE
//! the result lands (caller buffer / fresh allocation) and WHAT they return
//! (would-be length vs. bytes actually written).
//!
//! The string cores ([`vsnprintf_core`]/[`scnprintf_core`]) are kept free of the
//! C vararg ABI so they are host-testable with synthetic args. The `extern "C"`
//! entries forward their `...` into a `VaList`-typed sibling (the same idiom
//! relibc uses) so there is one vararg-reading path.
//!
//! `snprintf`/`sprintf`/`vsnprintf`/`vsprintf` shadow libc symbols, so — like
//! `string.rs` — they are gated out of the `hosttest` build to avoid colliding
//! with the host CRT; the kernel/bare-metal build always provides them. The
//! kernel-specific `scnprintf`/`vscnprintf`/`kasprintf`/`kvasprintf` do not
//! collide and stay available everywhere.

use crate::device::format_into;
use crate::mm;

/// Scratch cap for one formatted line (also the `sprintf` unbounded ceiling).
const TMP: usize = 1024;

/// NUL-terminated C string as a byte slice, capped at [`TMP`].
#[inline]
unsafe fn fmt_slice<'a>(fmt: *const u8) -> &'a [u8] {
    if fmt.is_null() {
        return &[];
    }
    let mut n = 0;
    while n < TMP && *fmt.add(n) != 0 {
        n += 1;
    }
    core::slice::from_raw_parts(fmt, n)
}

/// Format `fmt` into `buf` (size-bounded, NUL-terminated), returning the
/// would-be length (`snprintf` semantics: the length excluding NUL that a large
/// enough buffer would have held). Vararg-ABI-free for host testing.
pub fn vsnprintf_core(buf: *mut u8, size: usize, fmt: &[u8], next: impl FnMut(bool) -> u64) -> i32 {
    let mut tmp = [0u8; TMP];
    let len = format_into(&mut tmp, fmt, next);
    if !buf.is_null() && size > 0 {
        let copy = len.min(size - 1);
        unsafe {
            core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf, copy);
            *buf.add(copy) = 0;
        }
    }
    len as i32
}

/// Like [`vsnprintf_core`] but returns the number of bytes ACTUALLY written
/// (excluding NUL) — `scnprintf` semantics, capped at `size - 1`.
pub fn scnprintf_core(buf: *mut u8, size: usize, fmt: &[u8], next: impl FnMut(bool) -> u64) -> i32 {
    let would = vsnprintf_core(buf, size, fmt, next);
    if size == 0 {
        0
    } else {
        would.min((size - 1) as i32)
    }
}

#[inline]
unsafe fn rd(ap: &mut core::ffi::VaList, w64: bool) -> u64 {
    if w64 {
        ap.next_arg::<u64>()
    } else {
        ap.next_arg::<u32>() as u64
    }
}

// ── kernel-specific (no libc collision) — available in every build ───────────

/// `vscnprintf(buf, size, fmt, ap)`.
#[no_mangle]
pub unsafe extern "C" fn vscnprintf(
    buf: *mut u8,
    size: usize,
    fmt: *const u8,
    mut ap: core::ffi::VaList,
) -> i32 {
    scnprintf_core(buf, size, fmt_slice(fmt), |w64| rd(&mut ap, w64))
}

/// `scnprintf(buf, size, fmt, ...)`.
#[no_mangle]
pub unsafe extern "C" fn scnprintf(buf: *mut u8, size: usize, fmt: *const u8, args: ...) -> i32 {
    vscnprintf(buf, size, fmt, args)
}

/// `kvasprintf(gfp, fmt, ap)` — format into a freshly `kmalloc`'d, NUL-terminated
/// buffer; returns it (NULL on OOM). Caller frees with `kfree`.
#[no_mangle]
pub unsafe extern "C" fn kvasprintf(
    _gfp: u32,
    fmt: *const u8,
    mut ap: core::ffi::VaList,
) -> *mut u8 {
    let mut tmp = [0u8; TMP];
    let len = format_into(&mut tmp, fmt_slice(fmt), |w64| rd(&mut ap, w64));
    let out = mm::kmalloc(len + 1, 0);
    if !out.is_null() {
        core::ptr::copy_nonoverlapping(tmp.as_ptr(), out, len);
        *out.add(len) = 0;
    }
    out
}

/// `kasprintf(gfp, fmt, ...)`.
#[no_mangle]
pub unsafe extern "C" fn kasprintf(gfp: u32, fmt: *const u8, args: ...) -> *mut u8 {
    kvasprintf(gfp, fmt, args)
}

// ── libc-shadow names — gated out of the host harness (CRT collision) ────────

/// `vsnprintf(buf, size, fmt, ap)`.
#[cfg(any(not(feature = "hosttest"), feature = "hostrun"))]
#[no_mangle]
pub unsafe extern "C" fn vsnprintf(
    buf: *mut u8,
    size: usize,
    fmt: *const u8,
    mut ap: core::ffi::VaList,
) -> i32 {
    vsnprintf_core(buf, size, fmt_slice(fmt), |w64| rd(&mut ap, w64))
}

/// `snprintf(buf, size, fmt, ...)`.
#[cfg(any(not(feature = "hosttest"), feature = "hostrun"))]
#[no_mangle]
pub unsafe extern "C" fn snprintf(buf: *mut u8, size: usize, fmt: *const u8, args: ...) -> i32 {
    vsnprintf(buf, size, fmt, args)
}

/// `vsprintf(buf, fmt, ap)` — unbounded (capped at the scratch ceiling).
#[cfg(any(not(feature = "hosttest"), feature = "hostrun"))]
#[no_mangle]
pub unsafe extern "C" fn vsprintf(buf: *mut u8, fmt: *const u8, mut ap: core::ffi::VaList) -> i32 {
    vsnprintf_core(buf, TMP, fmt_slice(fmt), |w64| rd(&mut ap, w64))
}

/// `sprintf(buf, fmt, ...)`.
#[cfg(any(not(feature = "hosttest"), feature = "hostrun"))]
#[no_mangle]
pub unsafe extern "C" fn sprintf(buf: *mut u8, fmt: *const u8, args: ...) -> i32 {
    vsprintf(buf, fmt, args)
}
