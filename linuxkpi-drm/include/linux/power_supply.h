/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/power_supply.h> shim (MPL-2.0, original work).
 *
 * Battery/AC power-supply class. amdgpu_device queries power_supply_is_system_
 * supplied() to pick power profiles (AC vs battery). The query is backed by
 * raeen_linuxkpi at M4 (the daemon knows the real power source); declaration-only
 * here. License boundary (../../README.md): API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_POWER_SUPPLY_H
#define _LINUXKPI_LINUX_POWER_SUPPLY_H

#include <linux/types.h>

struct power_supply;
struct device;

enum power_supply_property {
	POWER_SUPPLY_PROP_STATUS,
	POWER_SUPPLY_PROP_ONLINE,
	POWER_SUPPLY_PROP_CAPACITY,
};

/* >0 = on AC, 0 = on battery, <0 = unknown — backed by raeen_linuxkpi (M4) */
int power_supply_is_system_supplied(void);

#endif /* _LINUXKPI_LINUX_POWER_SUPPLY_H */
