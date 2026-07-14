use crate::platform::types::*;
pub const TCGETS: c_ulong = 0x5401;
pub const TCSETS: c_ulong = 0x5402;
pub const TCSETSW: c_ulong = 0x5403;
pub const TCSETSF: c_ulong = 0x5404;
pub const TCSBRK: c_ulong = 0x5409;
pub const TCXONC: c_ulong = 0x540A;
pub const TCFLSH: c_ulong = 0x540B;
pub const TIOCSCTTY: c_ulong = 0x540E;
pub const TIOCGPGRP: c_ulong = 0x540F;
pub const TIOCSPGRP: c_ulong = 0x5410;
pub const TIOCGWINSZ: c_ulong = 0x5413;
pub const TIOCSWINSZ: c_ulong = 0x5414;
pub const TIOCGSID: c_ulong = 0x5429;
pub const FIONREAD: c_ulong = 0x541B;
pub const FIONBIO: c_ulong = 0x5421;
pub const TIOCSPTLCK: c_ulong = 0x4004_5431;
pub const TIOCGPTLCK: c_ulong = 0x8004_5439;
pub const SIOCATMARK: c_ulong = 0x8905;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ioctl(fd: c_int, request: c_ulong, out: *mut c_void) -> c_int {
    crate::platform::ERRNO.set(crate::header::errno::ENOSYS);
    -1
}
