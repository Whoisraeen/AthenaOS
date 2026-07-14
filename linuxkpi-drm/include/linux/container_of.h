/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/container_of.h> shim (MPL-2.0, original work).
 *
 * The canonical container_of() — recover the embedding struct from a pointer to
 * one of its members. The kernel split this out of <linux/kernel.h> into its own
 * header; several DRM helpers include it directly. Same definition as in list.h
 * (guarded, so they coexist). License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_CONTAINER_OF_H
#define _LINUXKPI_LINUX_CONTAINER_OF_H

#include <stddef.h>

#ifndef container_of
#define container_of(ptr, type, member) \
	((type *)((char *)(ptr) - offsetof(type, member)))
#endif

#ifndef container_of_const
#define container_of_const(ptr, type, member) container_of(ptr, type, member)
#endif

#endif /* _LINUXKPI_LINUX_CONTAINER_OF_H */
