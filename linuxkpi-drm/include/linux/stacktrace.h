/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/stacktrace.h> shim (MPL-2.0, original work).
 *
 * Kernel stack-trace capture. drm_mm.c includes this unconditionally but only
 * *uses* stack_trace_save()/stack_depot_save() inside its CONFIG_DRM_DEBUG_MM
 * block (hole-stamp debugging), which we do not enable. An empty surface is
 * therefore sufficient — no stack capture path is reached. The declarations are
 * provided for completeness should a caller reference them outside that guard.
 * License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_STACKTRACE_H
#define _LINUXKPI_LINUX_STACKTRACE_H

#include <linux/types.h>

struct task_struct;

unsigned int stack_trace_save(unsigned long *store, unsigned int size,
			      unsigned int skipnr);

#endif /* _LINUXKPI_LINUX_STACKTRACE_H */
