/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/syscalls.h> shim (MPL-2.0, original work).
 *
 * amdgpu_ras.c includes this for the ksys_sync_helper() prototype it calls on the
 * RAS error/reset path; it does NOT define any syscalls of its own. The helper is
 * backed by raeen_linuxkpi (a no-op on the bring-up daemon — see drm_bringup.rs).
 * License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_SYSCALLS_H
#define _LINUXKPI_LINUX_SYSCALLS_H

#include <linux/types.h>

/* drivers call this to flush the page cache before an emergency reset. */
int ksys_sync_helper(void);

#endif /* _LINUXKPI_LINUX_SYSCALLS_H */
