//! Linux timing — `jiffies` and `msleep` backed by kernel host syscalls.

use crate::host;

pub fn get_jiffies_64() -> u64 {
    unsafe { host::sys_linuxkpi_jiffies() }
}

pub fn msleep(msecs: u32) {
    unsafe {
        host::sys_linuxkpi_msleep(msecs as u64);
    }
}
