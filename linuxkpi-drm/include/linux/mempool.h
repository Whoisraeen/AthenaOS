/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/mempool.h> shim (MPL-2.0, original work).
 *
 * Pre-allocated memory pool (a reserve so an allocation can't fail under
 * pressure). The amd RAS subsystem keeps error-record pools here. Out of the MES
 * bring-up subset; backed by raeen_linuxkpi at M4 (a fake alloc returning NULL
 * would defeat the whole "can't fail" point; SCOPE.md rule 9). License boundary
 * (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_MEMPOOL_H
#define _LINUXKPI_LINUX_MEMPOOL_H

#include <linux/types.h>

typedef void *(*mempool_alloc_t)(gfp_t gfp_mask, void *pool_data);
typedef void  (*mempool_free_t)(void *element, void *pool_data);

typedef struct mempool_s {
	int    min_nr;
	int    curr_nr;
	void **elements;
	void  *pool_data;
	mempool_alloc_t alloc;
	mempool_free_t  free;
} mempool_t;

/* pool lifecycle + alloc/free — backed by raeen_linuxkpi (M4) */
mempool_t *mempool_create(int min_nr, mempool_alloc_t alloc_fn, mempool_free_t free_fn, void *pool_data);
void  mempool_destroy(mempool_t *pool);
void *mempool_alloc(mempool_t *pool, gfp_t gfp_mask);
void  mempool_free(void *element, mempool_t *pool);

/* common slab/kmalloc-backed pool helpers */
void *mempool_kmalloc(gfp_t gfp_mask, void *pool_data);
void  mempool_kfree(void *element, void *pool_data);
void *mempool_alloc_slab(gfp_t gfp_mask, void *pool_data);
void  mempool_free_slab(void *element, void *pool_data);

#endif /* _LINUXKPI_LINUX_MEMPOOL_H */
