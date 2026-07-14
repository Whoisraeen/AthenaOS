/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/firmware.h> shim (MPL). request_firmware/release_firmware surface —
 * amdgpu loads its ucode blobs through this. Backed by raeen_linuxkpi's firmware
 * facade at link time (M4); a declaration-only header for M2/M3 typecheck.
 */
#ifndef _LINUXKPI_LINUX_FIRMWARE_H
#define _LINUXKPI_LINUX_FIRMWARE_H

#include <linux/types.h>
/* MODULE_FIRMWARE() declares which firmware blobs a driver needs; the kernel
 * spells it in <linux/module.h>, and firmware consumers reach it transitively.
 * Pull it here so any firmware.h user (e.g. amdgpu_discovery.c, which includes
 * only firmware.h) sees the metadata no-op macros rather than a bare string. */
#include <linux/module.h>

struct device;
struct module;

struct firmware {
	size_t size;
	const u8 *data;
	void *priv;
};

int request_firmware(const struct firmware **fw, const char *name,
		     struct device *device);
int request_firmware_direct(const struct firmware **fw, const char *name,
			    struct device *device);
int firmware_request_nowarn(const struct firmware **fw, const char *name,
			    struct device *device);
void release_firmware(const struct firmware *fw);

#endif /* _LINUXKPI_LINUX_FIRMWARE_H */
