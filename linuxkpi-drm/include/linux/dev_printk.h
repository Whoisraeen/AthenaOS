/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/dev_printk.h> shim (MPL-2.0, original work).
 *
 * The dev_err/dev_warn/dev_info family. In the kernel this is the light header
 * that <linux/device.h> includes; our device.h already defines the dev_* family
 * (backed by the M4 log facade), so this re-exposes it for the headers (amd RAS
 * ras_sys.h) that include dev_printk.h directly. License boundary: API surface.
 */
#ifndef _LINUXKPI_LINUX_DEV_PRINTK_H
#define _LINUXKPI_LINUX_DEV_PRINTK_H

#include <linux/device.h>

#endif /* _LINUXKPI_LINUX_DEV_PRINTK_H */
