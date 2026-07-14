/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/kmemleak.h> shim (MPL-2.0, original work).
 *
 * kmemleak is the kernel's opt-in memory-leak detector; its hooks are compiled
 * to no-ops when CONFIG_DEBUG_KMEMLEAK is off (the default and our case). The
 * drm-core buddy allocator calls kmemleak_update_trace() after resizing a block
 * record; with no leak tracker active there is nothing to update. Faithful to
 * the CONFIG_DEBUG_KMEMLEAK=n contract, not a fake. License boundary
 * (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_KMEMLEAK_H
#define _LINUXKPI_LINUX_KMEMLEAK_H

#define kmemleak_update_trace(ptr)          do { } while (0)
#define kmemleak_alloc(ptr, size, mc, gfp)  do { } while (0)
#define kmemleak_free(ptr)                  do { } while (0)
#define kmemleak_ignore(ptr)                do { } while (0)
#define kmemleak_not_leak(ptr)              do { } while (0)
#define kmemleak_no_scan(ptr)               do { } while (0)

#endif /* _LINUXKPI_LINUX_KMEMLEAK_H */
