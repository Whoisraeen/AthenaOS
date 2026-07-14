use crate::platform::types::{c_int};
pub unsafe fn openpty(_name: &mut [u8]) -> Result<(c_int, c_int), ()> {
    crate::platform::ERRNO.set(crate::header::errno::ENOSYS);
    Err(())
}
