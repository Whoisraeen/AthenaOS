/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/mmzone.h> shim (MPL-2.0, original work).
 *
 * Page-allocator zone definitions. TTM's page pool sizes its per-order free
 * lists from MAX_ORDER/NR_PAGE_ORDERS. The MES path does not allocate BO pages
 * (TTM is stubbed per SCOPE.md), so this provides just the order constants the
 * type graph needs; `struct zone` stays opaque. License boundary: API surface.
 */
#ifndef _LINUXKPI_LINUX_MMZONE_H
#define _LINUXKPI_LINUX_MMZONE_H

#include <linux/types.h>

/* Buddy-allocator max order: orders 0..MAX_ORDER, NR_PAGE_ORDERS buckets. */
#ifndef MAX_ORDER
#define MAX_ORDER 10
#endif
#ifndef NR_PAGE_ORDERS
#define NR_PAGE_ORDERS (MAX_ORDER + 1)
#endif
#ifndef MAX_PAGE_ORDER
#define MAX_PAGE_ORDER MAX_ORDER
#endif

struct zone;
struct pglist_data;

enum zone_type {
	ZONE_DMA,
	ZONE_DMA32,
	ZONE_NORMAL,
	ZONE_MOVABLE,
	__MAX_NR_ZONES,
};

#endif /* _LINUXKPI_LINUX_MMZONE_H */
