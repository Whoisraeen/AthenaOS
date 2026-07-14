/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/apple-gmux.h> shim (MPL-2.0, original work).
 *
 * Apple GMUX dual-GPU mux (MacBook Pro). amdgpu_device checks for it to cooperate
 * with the firmware on Apple hardware. Athena is not a Mac, so this honestly
 * reports "not present". License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_APPLE_GMUX_H
#define _LINUXKPI_LINUX_APPLE_GMUX_H

#include <linux/types.h>

static inline bool apple_gmux_detect(void *pdev, bool *indexed_ret) { (void)pdev; (void)indexed_ret; return false; }
static inline bool apple_gmux_present(void) { return false; }

#endif /* _LINUXKPI_LINUX_APPLE_GMUX_H */
