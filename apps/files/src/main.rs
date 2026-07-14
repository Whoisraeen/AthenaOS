//! AthenaOS File Manager — userspace ELF entry shim.
//!
//! All the app (state, draw path, syscall wiring, host-renderable `render_preview`)
//! lives in the `files` LIBRARY crate (`src/lib.rs`) so the host screenshot
//! harness can link it and render the LIVE Files draw path with no kernel. This
//! bin is just the freestanding `_start` that hands control to `files::run()`.

#![no_std]
#![no_main]

#[no_mangle]
pub extern "C" fn _start() -> ! {
    files::run()
}
