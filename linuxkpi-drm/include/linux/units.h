/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/units.h> shim (MPL-2.0, original work).
 *
 * Unit-conversion constants. amdgpu's SMU/power code converts Hz/W/temperature
 * with these. Pure constants. License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_UNITS_H
#define _LINUXKPI_LINUX_UNITS_H

#define HZ_PER_KHZ          1000UL
#define HZ_PER_MHZ          1000000UL
#define KHZ_PER_MHZ         1000UL
#define MILLIWATT_PER_WATT  1000L
#define MICROWATT_PER_MILLIWATT 1000L
#define MICROWATT_PER_WATT  1000000L

#define NANO   1000000000ULL
#define MICRO  1000000UL
#define MILLI  1000UL
#define KILO   1000UL
#define MEGA   1000000UL
#define GIGA   1000000000ULL

/* temperature conversions (milli-degrees) */
#define MILLIDEGREE_PER_DEGREE      1000
#define MILLIDEGREE_PER_DECIDEGREE  100
#define ABSOLUTE_ZERO_MILLICELSIUS  (-273150)

#endif /* _LINUXKPI_LINUX_UNITS_H */
