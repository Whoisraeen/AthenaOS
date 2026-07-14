//! relibc link + POSIX smoke test daemon (spawned by user_init as `hello_relibc`).

#![no_std]
#![no_main]

use core::ffi::{c_char, c_int};

extern "C" {
    fn printf(fmt: *const u8, ...) -> c_int;
    fn open(path: *const u8, flags: c_int, mode: c_int) -> c_int;
    fn read(fd: c_int, buf: *mut u8, count: usize) -> isize;
    fn close(fd: c_int) -> c_int;
    fn pipe(fds: *mut c_int) -> c_int;
    fn signal(sig: c_int, handler: usize) -> usize;
}

const O_RDONLY: c_int = 0x0001_0000;

#[no_mangle]
pub extern "C" fn main(_argc: isize, _argv: *mut *mut c_char, _envp: *mut *mut c_char) -> c_int {
    unsafe {
        printf(b"[hello_relibc] relibc printf OK\n\0".as_ptr());
    }
    run_posix_smoke();
    0
}

fn run_posix_smoke() {
    unsafe {
        // File I/O: read a procfs line via relibc open/read/close.
        let fd = open(b"/proc/version\0".as_ptr(), O_RDONLY, 0);
        if fd < 0 {
            printf(b"[hello_relibc] POSIX file: open /proc/version FAILED\n\0".as_ptr());
        } else {
            let mut buf = [0u8; 64];
            let n = read(fd, buf.as_mut_ptr(), buf.len());
            close(fd);
            if n > 0 {
                printf(
                    b"[hello_relibc] POSIX file: read %d bytes from /proc/version\n\0".as_ptr(),
                    n as c_int,
                );
            } else {
                printf(b"[hello_relibc] POSIX file: read FAILED\n\0".as_ptr());
            }
        }

        // Pipes: not implemented on AthenaOS yet — expect failure.
        let mut fds = [0i32; 2];
        if pipe(fds.as_mut_ptr()) == 0 {
            printf(b"[hello_relibc] POSIX pipe: unexpected success\n\0".as_ptr());
            close(fds[0]);
            close(fds[1]);
        } else {
            printf(b"[hello_relibc] POSIX pipe: ENOSYS (expected)\n\0".as_ptr());
        }

        // Signals: adapter returns ENOSYS until kernel signal delivery lands.
        if signal(2, 0) == usize::MAX {
            printf(b"[hello_relibc] POSIX signal: ENOSYS (expected)\n\0".as_ptr());
        } else {
            printf(b"[hello_relibc] POSIX signal: handler query OK\n\0".as_ptr());
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
