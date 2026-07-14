/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/mod_devicetable.h> shim (MPL-2.0, original work).
 *
 * The driver<->device match-table id structs. amdgpu's primary match is
 * `struct pci_device_id` (defined in <linux/pci.h> in this shim); the platform/
 * ACPI/OF id structs here cover the SoC-variant and helper declarations. License
 * boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_MOD_DEVICETABLE_H
#define _LINUXKPI_LINUX_MOD_DEVICETABLE_H

#include <linux/types.h>

struct platform_device_id {
	char          name[20];
	unsigned long driver_data;
};

struct acpi_device_id {
	char          id[16];
	unsigned long driver_data;
	unsigned int  cls;
	unsigned int  cls_msk;
};

struct of_device_id {
	char        name[32];
	char        type[32];
	char        compatible[128];
	const void *data;
};

struct i2c_device_id {
	char          name[20];
	unsigned long driver_data;
};

#endif /* _LINUXKPI_LINUX_MOD_DEVICETABLE_H */
