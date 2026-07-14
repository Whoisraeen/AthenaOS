//! `athena_printk` — forwards a C string to the kernel serial log, plus the
//! printf-style driver/DRM logging entry points (`_dev_printk`, `__drm_err`,
//! `drm_dev_printk`, `drm_printf`). The latter format their varargs via the
//! crate's `vscnprintf` (printf.rs) into a stack buffer, then forward to the log
//! — mirroring the `scnprintf`→`vscnprintf` variadic idiom used there.

use crate::host;
use core::ffi::c_void;

pub fn athena_printk(msg: &[u8]) -> i32 {
    let len = msg.iter().position(|&b| b == 0).unwrap_or(msg.len());
    if len == 0 {
        return 0;
    }
    let ok = unsafe { host::sys_linuxkpi_printk(msg.as_ptr(), len as u64) };
    if ok == 0 {
        0
    } else {
        -1
    }
}

/// `_dev_printk(level, dev, fmt, ...)` — device-scoped kernel message.
#[no_mangle]
pub unsafe extern "C" fn _dev_printk(
    _level: *const u8,
    _dev: *const c_void,
    fmt: *const u8,
    args: ...
) {
    let mut buf = [0u8; 512];
    let n = crate::printf::vscnprintf(buf.as_mut_ptr(), buf.len(), fmt, args);
    if n > 0 {
        athena_printk(&buf[..(n as usize).min(buf.len())]);
    }
}

/// `__drm_err(fmt, ...)` — DRM error message.
#[no_mangle]
pub unsafe extern "C" fn __drm_err(fmt: *const u8, args: ...) {
    let mut buf = [0u8; 512];
    let n = crate::printf::vscnprintf(buf.as_mut_ptr(), buf.len(), fmt, args);
    if n > 0 {
        athena_printk(&buf[..(n as usize).min(buf.len())]);
    }
}

/// `drm_dev_printk(dev, level, fmt, ...)` — DRM device-scoped message.
#[no_mangle]
pub unsafe extern "C" fn drm_dev_printk(
    _dev: *const c_void,
    _level: *const u8,
    fmt: *const u8,
    args: ...
) {
    let mut buf = [0u8; 512];
    let n = crate::printf::vscnprintf(buf.as_mut_ptr(), buf.len(), fmt, args);
    if n > 0 {
        athena_printk(&buf[..(n as usize).min(buf.len())]);
    }
}

/// `drm_printf(printer, fmt, ...)` — DRM uses a `struct drm_printer` with a sink
/// callback; the daemon routes straight to the serial log.
#[no_mangle]
pub unsafe extern "C" fn drm_printf(_p: *const c_void, fmt: *const u8, args: ...) {
    let mut buf = [0u8; 512];
    let n = crate::printf::vscnprintf(buf.as_mut_ptr(), buf.len(), fmt, args);
    if n > 0 {
        athena_printk(&buf[..(n as usize).min(buf.len())]);
    }
}

/// `__drm_dev_dbg(desc, dev, category, fmt, ...)` — DRM debug message.
#[no_mangle]
pub unsafe extern "C" fn __drm_dev_dbg(
    _desc: *const c_void,
    _dev: *const c_void,
    _category: u32,
    fmt: *const u8,
    args: ...
) {
    let mut buf = [0u8; 512];
    let n = crate::printf::vscnprintf(buf.as_mut_ptr(), buf.len(), fmt, args);
    if n > 0 {
        athena_printk(&buf[..(n as usize).min(buf.len())]);
    }
}
