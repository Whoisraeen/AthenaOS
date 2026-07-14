/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/ioport.h> shim (MPL-2.0, original work).
 *
 * I/O + memory resource descriptors. `struct resource` is how amdgpu's PCI BARs
 * are described (start/end/flags). Pure type + flag constants. License boundary
 * (../../README.md): API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_IOPORT_H
#define _LINUXKPI_LINUX_IOPORT_H

#include <linux/types.h>

struct resource {
	resource_size_t start;
	resource_size_t end;
	const char     *name;
	unsigned long   flags;
	unsigned long   desc;
	struct resource *parent, *sibling, *child;
};

#define IORESOURCE_IO        0x00000100
#define IORESOURCE_MEM       0x00000200
#define IORESOURCE_IRQ       0x00000400
#define IORESOURCE_DMA       0x00000800
#define IORESOURCE_MEM_64    0x00100000
#define IORESOURCE_PREFETCH  0x00002000
#define IORESOURCE_READONLY  0x00004000
#define IORESOURCE_ROM_SHADOW 0x00020000  /* ROM is a copy of the BIOS shadow */
#define IORESOURCE_BUSY      0x80000000
#define IORESOURCE_DISABLED  0x10000000
#define IORESOURCE_UNSET     0x20000000
#define IORESOURCE_TYPE_BITS 0x00001f00

static inline resource_size_t resource_size(const struct resource *res)
{ return res->end - res->start + 1; }

#endif /* _LINUXKPI_LINUX_IOPORT_H */
