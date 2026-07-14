/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/kgdb.h> shim (MPL-2.0, original work).
 *
 * Kernel GDB stub. amdgpu's DC os_types.h checks in_dbg_master() to soften some
 * asserts when a debugger is attached. There is no kgdb in the userspace-daemon
 * model, so in_dbg_master() honestly reports 0 (never attached) and the
 * breakpoint hook is a no-op — not a fake of a functional path, just "no kernel
 * debugger present". License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_KGDB_H
#define _LINUXKPI_LINUX_KGDB_H

#include <linux/types.h>

static inline int  in_dbg_master(void) { return 0; }
static inline void kgdb_breakpoint(void) { }

#endif /* _LINUXKPI_LINUX_KGDB_H */
