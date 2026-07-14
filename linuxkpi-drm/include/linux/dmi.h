/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/dmi.h> shim (MPL-2.0, original work).
 *
 * DMI/SMBIOS table access. KFD topology reads the system manufacturer/product to
 * apply per-platform quirks. Backed by raeen_linuxkpi at M4 (the daemon can supply
 * the real SMBIOS strings); declaration-only here. A NULL return is honest "field
 * not present", which callers already handle. License boundary: API surface.
 */
#ifndef _LINUXKPI_LINUX_DMI_H
#define _LINUXKPI_LINUX_DMI_H

#include <linux/types.h>

/* raw SMBIOS structure header (kfd topology walks DMI memory-device entries). */
struct dmi_header {
	u8  type;
	u8  length;
	u16 handle;
};

enum dmi_field {
	DMI_NONE,
	DMI_BIOS_VENDOR, DMI_BIOS_VERSION, DMI_BIOS_DATE,
	DMI_SYS_VENDOR, DMI_PRODUCT_NAME, DMI_PRODUCT_VERSION, DMI_PRODUCT_SERIAL,
	DMI_BOARD_VENDOR, DMI_BOARD_NAME, DMI_BOARD_VERSION,
	DMI_STRING_MAX,
};

struct dmi_strmatch { unsigned char slot; char substr[79]; };
struct dmi_system_id {
	int (*callback)(const struct dmi_system_id *);
	const char *ident;
	struct dmi_strmatch matches[4];
	void *driver_data;
};

const char *dmi_get_system_info(int field);
int  dmi_check_system(const struct dmi_system_id *list);
bool dmi_match(enum dmi_field f, const char *str);

#endif /* _LINUXKPI_LINUX_DMI_H */
