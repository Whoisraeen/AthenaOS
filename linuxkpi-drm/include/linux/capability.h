/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/capability.h> shim (MPL-2.0, original work).
 *
 * POSIX capability constants + the capable() privilege check. amdgpu gates a few
 * actions on these (e.g. CAP_SYS_NICE for a high-priority GPU context). On
 * RaeenOS the real authority is `crate::capability` (the Cap enum) — capable() is
 * backed by raeen_linuxkpi at M4, mapping these onto the daemon's RaeShield grant.
 * License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_CAPABILITY_H
#define _LINUXKPI_LINUX_CAPABILITY_H

#include <linux/types.h>

#define CAP_IPC_LOCK      14
#define CAP_SYS_ADMIN     21
#define CAP_SYS_NICE      23
#define CAP_SYS_RESOURCE  24

/* privilege checks — backed by raeen_linuxkpi (M4, via RaeShield) */
bool capable(int cap);
bool perfmon_capable(void);

#endif /* _LINUXKPI_LINUX_CAPABILITY_H */
