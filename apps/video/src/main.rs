//! AthenaOS Video — userspace ELF entry shim.
//!
//! All of the app (state, the open/demux pipeline, the decode dispatch, the draw
//! path, the syscall wiring) lives in the `video` LIBRARY crate (`src/lib.rs`) so a
//! host KAT can link it and exercise the LIVE engines (`rae_mp4` demuxer +
//! `raemedia` H264/AAC decoders) with no kernel. This bin is just the freestanding
//! `_start` that hands control to `video::run()`.

#![no_std]
#![no_main]

#[no_mangle]
pub extern "C" fn _start() -> ! {
    video::run()
}
