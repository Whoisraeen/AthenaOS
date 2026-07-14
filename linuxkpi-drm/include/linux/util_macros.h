/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/util_macros.h> shim (MPL-2.0, original work).
 *
 * "Find the nearest table entry" helpers used by clock/voltage table lookups.
 * Pure macros — the obvious nearest-neighbour scan the names dictate. License
 * boundary (../../README.md): API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_UTIL_MACROS_H
#define _LINUXKPI_LINUX_UTIL_MACROS_H

/* index of the array entry closest to x (ascending array). */
#define find_closest(x, a, as) \
	({ \
		__typeof__(as) __fc_i, __fc_as = (as); \
		__typeof__(x) __fc_x = (x); \
		for (__fc_i = 0; __fc_i < __fc_as - 1; __fc_i++) \
			if (__fc_x <= ((a)[__fc_i] + (a)[__fc_i + 1] + 1) / 2) \
				break; \
		__fc_i; \
	})

#define find_closest_descending(x, a, as) \
	({ \
		__typeof__(as) __fc_i, __fc_as = (as); \
		__typeof__(x) __fc_x = (x); \
		for (__fc_i = 0; __fc_i < __fc_as - 1; __fc_i++) \
			if (__fc_x >= ((a)[__fc_i] + (a)[__fc_i + 1] + 1) / 2) \
				break; \
		__fc_i; \
	})

#endif /* _LINUXKPI_LINUX_UTIL_MACROS_H */
