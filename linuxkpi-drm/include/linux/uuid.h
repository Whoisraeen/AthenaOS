/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/uuid.h> shim (MPL-2.0, original work).
 *
 * 128-bit GUID/UUID. The DisplayPort-MST topology code keys branch devices by
 * guid_t. A 16-byte blob (kernel ABI). Reached via the display type graph for
 * layout; out of the MES subset. License boundary (../../README.md): surface.
 */
#ifndef _LINUXKPI_LINUX_UUID_H
#define _LINUXKPI_LINUX_UUID_H

#include <linux/types.h>   /* guid_t / uuid_t live in the universal base */

#define UUID_SIZE 16

static inline void guid_copy(guid_t *dst, const guid_t *src) { *dst = *src; }
static inline bool guid_equal(const guid_t *a, const guid_t *b)
{ int i; for (i = 0; i < UUID_SIZE; i++) if (a->b[i] != b->b[i]) return false; return true; }

#endif /* _LINUXKPI_LINUX_UUID_H */
