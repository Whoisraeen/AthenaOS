/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/stackdepot.h> shim (MPL-2.0, original work).
 *
 * Saved-stack-trace depot — a DEBUG aid (lockdep/KASAN/WW-mutex blame). Reached
 * only via the DRM modeset-lock type graph that amdgpu_device's embedded
 * drm_device drags in for layout; no functional GPU path uses it. Built OFF (the
 * CONFIG_STACKDEPOT=n posture): saving returns the null handle, fetching yields
 * nothing. This is honest debug-disabled, not a fake of a functional path.
 * License boundary (../../README.md): API surface only.
 */
#ifndef _LINUXKPI_LINUX_STACKDEPOT_H
#define _LINUXKPI_LINUX_STACKDEPOT_H

#include <linux/types.h>

typedef u32 depot_stack_handle_t;

#define STACK_DEPOT_FLAG_CAN_ALLOC  0x0001
#define STACK_DEPOT_FLAG_GET        0x0002

static inline depot_stack_handle_t stack_depot_save(const unsigned long *entries,
						    unsigned int nr_entries, gfp_t gfp)
{ (void)entries; (void)nr_entries; (void)gfp; return 0; }

static inline unsigned int stack_depot_fetch(depot_stack_handle_t handle,
					     unsigned long **entries)
{ (void)handle; if (entries) *entries = (void *)0; return 0; }

static inline int  stack_depot_init(void) { return 0; }
static inline void stack_depot_print(depot_stack_handle_t stack) { (void)stack; }
static inline int  stack_depot_snprint(depot_stack_handle_t handle, char *buf, size_t size, int spaces)
{ (void)handle; (void)spaces; if (buf && size) buf[0] = 0; return 0; }

#endif /* _LINUXKPI_LINUX_STACKDEPOT_H */
