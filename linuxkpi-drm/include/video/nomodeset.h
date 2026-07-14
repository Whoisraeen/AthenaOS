/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <video/nomodeset.h> shim (MPL-2.0, original work).
 *
 * The `nomodeset` boot-flag query — drm_drv.h checks it to refuse loading when the
 * user booted with modesetting disabled. In the RaeenOS daemon model there is no
 * such boot flag, so it honestly reports false (modesetting is allowed). Not a
 * fake of a functional path. License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_VIDEO_NOMODESET_H
#define _LINUXKPI_VIDEO_NOMODESET_H

#include <linux/types.h>

/* NOTE: drm_drv.h itself provides the drm_firmware_drivers_only() stub when
 * CONFIG_VIDEO_NOMODESET is unset (our case), so we must NOT define it here — this
 * header only needs to EXIST so the include resolves. */

#endif /* _LINUXKPI_VIDEO_NOMODESET_H */
