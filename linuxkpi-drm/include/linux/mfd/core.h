/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/mfd/core.h> shim (MPL-2.0, original work).
 *
 * Multi-function device cells. amdgpu uses this only for the ACP (Audio
 * CoProcessor) sub-device registration — out of the MES bring-up subset
 * (SCOPE.md). Reached via amdgpu_acp.h for type/decl layout; the add/remove ops
 * are backed by ath_linuxkpi at M4 if ACP is ever brought into scope. License
 * boundary (../../README.md): API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_MFD_CORE_H
#define _LINUXKPI_LINUX_MFD_CORE_H

#include <linux/types.h>

struct device;
struct platform_device;
struct resource;

struct mfd_cell {
	const char *name;
	int         id;
	int         num_resources;
	const struct resource *resources;
	void       *platform_data;
	size_t      pdata_size;
	const char *of_compatible;
};

int  mfd_add_devices(struct device *parent, int id, const struct mfd_cell *cells,
		     int n_devs, struct resource *mem_base, int irq_base, void *domain);
int  mfd_add_hotplug_devices(struct device *parent, const struct mfd_cell *cells, int n_devs);
void mfd_remove_devices(struct device *parent);

#endif /* _LINUXKPI_LINUX_MFD_CORE_H */
