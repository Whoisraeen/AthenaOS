/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/iosys-map.h> shim (MPL-2.0, original work).
 *
 * Tagged pointer abstracting "this mapping is system RAM vs MMIO". TTM/DRM use it
 * for BO kmaps. REAL inline helpers — pointer arithmetic + builtin memcpy; on
 * x86 userspace an ioremap'd region is an ordinary mapped pointer (the M4 facade
 * returns one), so the iomem and system paths coincide. Not a fake: the accessors
 * genuinely read/write the mapped bytes. License boundary (../../README.md):
 * API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_IOSYS_MAP_H
#define _LINUXKPI_LINUX_IOSYS_MAP_H

#include <linux/types.h>

struct iosys_map {
	union {
		void *vaddr_iomem;
		void *vaddr;
	};
	bool is_iomem;
};

#define IOSYS_MAP_INIT_VADDR(vaddr_) { .vaddr = (vaddr_), .is_iomem = false }

static inline void iosys_map_set_vaddr(struct iosys_map *map, void *vaddr)
{ map->vaddr = vaddr; map->is_iomem = false; }
static inline void iosys_map_set_vaddr_iomem(struct iosys_map *map, void *vaddr_iomem)
{ map->vaddr_iomem = vaddr_iomem; map->is_iomem = true; }
static inline void iosys_map_clear(struct iosys_map *map) { map->vaddr = (void *)0; map->is_iomem = false; }
static inline bool iosys_map_is_null(const struct iosys_map *map) { return !map->vaddr; }
static inline bool iosys_map_is_set(const struct iosys_map *map)  { return map->vaddr != (void *)0; }
static inline bool iosys_map_is_equal(const struct iosys_map *a, const struct iosys_map *b)
{ return a->is_iomem == b->is_iomem && a->vaddr == b->vaddr; }
static inline void iosys_map_incr(struct iosys_map *map, size_t incr) { map->vaddr = (char *)map->vaddr + incr; }

static inline void iosys_map_memcpy_to(struct iosys_map *dst, size_t off, const void *src, size_t len)
{ __builtin_memcpy((char *)dst->vaddr + off, src, len); }
static inline void iosys_map_memcpy_from(void *dst, const struct iosys_map *src, size_t off, size_t len)
{ __builtin_memcpy(dst, (const char *)src->vaddr + off, len); }
static inline void iosys_map_memset(struct iosys_map *dst, size_t off, int value, size_t len)
{ __builtin_memset((char *)dst->vaddr + off, value, len); }

#define iosys_map_rd(map_, offset_, type_) \
	(*(type_ *)((char *)(map_)->vaddr + (offset_)))
#define iosys_map_wr(map_, offset_, type_, val_) \
	do { *(type_ *)((char *)(map_)->vaddr + (offset_)) = (val_); } while (0)

#endif /* _LINUXKPI_LINUX_IOSYS_MAP_H */
