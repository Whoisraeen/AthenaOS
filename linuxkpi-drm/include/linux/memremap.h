/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/memremap.h> shim (MPL-2.0, original work).
 *
 * Device-memory remapping (ZONE_DEVICE / device-private pages for SVM). Not on
 * the MES bring-up path (SVM is out of subset); reached via amdgpu_amdkfd.h for
 * type/decl layout. Backed by raeen_linuxkpi at M4 if SVM is brought into scope.
 * License boundary (../../README.md): API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_MEMREMAP_H
#define _LINUXKPI_LINUX_MEMREMAP_H

#include <linux/types.h>

struct device;
struct page;

/* memremap() cache-mode flags */
#define MEMREMAP_WB  (1 << 0)
#define MEMREMAP_WT  (1 << 1)
#define MEMREMAP_WC  (1 << 2)
#define MEMREMAP_ENC (1 << 3)
#define MEMREMAP_DEC (1 << 4)

enum memory_type {
	MEMORY_DEVICE_PRIVATE = 1,
	MEMORY_DEVICE_COHERENT,
	MEMORY_DEVICE_FS_DAX,
	MEMORY_DEVICE_GENERIC,
	MEMORY_DEVICE_PCI_P2PDMA,
};

struct range { u64 start; u64 end; };

struct dev_pagemap_ops;

struct dev_pagemap {
	enum memory_type type;
	unsigned int     flags;
	const struct dev_pagemap_ops *ops;
	void            *owner;
	int              nr_range;
	struct range     range;
};

/* backed by raeen_linuxkpi (M4) */
void *devm_memremap_pages(struct device *dev, struct dev_pagemap *pgmap);
void  memunmap_pages(struct dev_pagemap *pgmap);
void *memremap(resource_size_t offset, size_t size, unsigned long flags);
void  memunmap(void *addr);

#endif /* _LINUXKPI_LINUX_MEMREMAP_H */
