/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/spinlock_types.h> shim (MPL-2.0, original work).
 *
 * In the kernel this carries just the spinlock_t/raw_spinlock_t type definitions
 * (split out to break include cycles). Our spinlock.h has no such cycle (it only
 * needs <linux/types.h>), so this simply re-exposes those types. License boundary
 * (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_SPINLOCK_TYPES_H
#define _LINUXKPI_LINUX_SPINLOCK_TYPES_H

#include <linux/spinlock.h>

#endif /* _LINUXKPI_LINUX_SPINLOCK_TYPES_H */
