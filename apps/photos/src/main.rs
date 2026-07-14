//! RaeenOS Photos — userspace ELF entry shim.
//!
//! All the app (state, draw path, syscall wiring, the syscall-free decode
//! dispatch + the design/pipeline proofs) lives in the `photos` LIBRARY crate
//! (`src/lib.rs`) so a host KAT can link it and exercise the LIVE image decoders
//! (rae_image + rae_gif) with no kernel. This bin is just the freestanding
//! `_start` that hands control to `photos::run()`.

#![no_std]
#![no_main]

#[no_mangle]
pub extern "C" fn _start() -> ! {
    photos::run()
}
