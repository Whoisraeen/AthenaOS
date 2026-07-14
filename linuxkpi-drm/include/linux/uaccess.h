/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/uaccess.h> shim (MPL-2.0, original work).
 *
 * The kernel<->user copy boundary. amdgpu uses copy_from_user/copy_to_user in its
 * ioctl handlers (mostly the ioctl path, peripheral to MES bring-up). In the
 * AthenaOS daemon model the "user" is the guest app across a capability IPC; the
 * copies are backed by raeen_linuxkpi at M4 (a fake that did not actually copy
 * would silently corrupt every ioctl arg; SCOPE.md rule 9). License boundary
 * (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_UACCESS_H
#define _LINUXKPI_LINUX_UACCESS_H

#include <linux/types.h>

#define access_ok(addr, size) (1)

/* return the number of bytes NOT copied (0 = full success) -- backed by M4. */
unsigned long copy_to_user(void __user *to, const void *from, unsigned long n);
unsigned long copy_from_user(void *to, const void __user *from, unsigned long n);
unsigned long clear_user(void __user *to, unsigned long n);
void *memdup_user(const void __user *src, size_t len);
void *memdup_user_nul(const void __user *src, size_t len);

#define get_user(x, ptr)  ({ (x) = *(ptr); 0; })
#define put_user(x, ptr)  ({ *(ptr) = (x); 0; })

/* cast a __u64 from a uapi struct to a userspace pointer (ioctl arg passing). */
#define u64_to_user_ptr(x) ((void __user *)(uintptr_t)(x))
/* address-tagging strip (ARM MTE / x86 LAM) — a no-op on the bring-up target. */
#ifndef untagged_addr
#define untagged_addr(addr) (addr)
#endif

#endif /* _LINUXKPI_LINUX_UACCESS_H */
