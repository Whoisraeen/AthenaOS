/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/io-mapping.h> shim (MPL-2.0, original work).
 *
 * Bounded write-combined MMIO windows. TTM's resource layer uses an io_mapping to
 * peek at VRAM through the visible aperture. The map/unmap route to the host MMIO
 * facade at M5; the type is laid out for by-value embeds. License boundary
 * (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_IO_MAPPING_H
#define _LINUXKPI_LINUX_IO_MAPPING_H

#include <linux/types.h>
#include <linux/io.h>

struct io_mapping {
	resource_size_t base;
	unsigned long   size;
	void __iomem   *iomem;
	unsigned long   prot;
};

/* map/unmap — backed by raeen_linuxkpi (M4) */
void *io_mapping_map_wc(struct io_mapping *mapping, unsigned long offset, unsigned long size);
void  io_mapping_unmap(void *vaddr);
void *io_mapping_map_local_wc(struct io_mapping *mapping, unsigned long offset);
void  io_mapping_unmap_local(void *vaddr);
struct io_mapping *io_mapping_init_wc(struct io_mapping *iomap, resource_size_t base, unsigned long size);
void  io_mapping_fini(struct io_mapping *mapping);

#endif /* _LINUXKPI_LINUX_IO_MAPPING_H */
