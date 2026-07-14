/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/radix-tree.h> shim (MPL-2.0, original work).
 *
 * Sparse pointer array keyed by an unsigned long (the structure xarray wraps).
 * amdgpu RAS keeps per-address error records here. Init is a pure reset (inline);
 * the insert/lookup/delete + iteration are a real tree backed by ath_linuxkpi at
 * M4 — not faked to "always empty" (SCOPE.md rule 9). License boundary: surface.
 */
#ifndef _LINUXKPI_LINUX_RADIX_TREE_H
#define _LINUXKPI_LINUX_RADIX_TREE_H

#include <linux/types.h>

struct radix_tree_root {
	gfp_t  gfp_mask;
	void  *rnode;
};
struct radix_tree_iter {
	unsigned long index;
	unsigned long next_index;
	unsigned long tags;
	void        **slot;
};

#define INIT_RADIX_TREE(root, mask) do { (root)->gfp_mask = (mask); (root)->rnode = (void *)0; } while (0)
#define RADIX_TREE(name, mask)      struct radix_tree_root name = { .gfp_mask = (mask), .rnode = (void *)0 }
#define RADIX_TREE_INIT(name, mask) { .gfp_mask = (mask), .rnode = (void *)0 }

/* data ops — backed by ath_linuxkpi (M4) */
int   radix_tree_insert(struct radix_tree_root *root, unsigned long index, void *item);
void *radix_tree_lookup(const struct radix_tree_root *root, unsigned long index);
void *radix_tree_delete(struct radix_tree_root *root, unsigned long index);
void **radix_tree_iter_init(struct radix_tree_iter *iter, unsigned long start);
void **radix_tree_next_chunk(const struct radix_tree_root *root, struct radix_tree_iter *iter, unsigned int flags);
unsigned int radix_tree_gang_lookup(const struct radix_tree_root *root, void **results,
				    unsigned long first_index, unsigned int max_items);

static inline void *radix_tree_deref_slot(void **slot) { return slot ? *slot : (void *)0; }

#define radix_tree_for_each_slot(slot, root, iter, start) \
	for ((slot) = radix_tree_iter_init((iter), (start)); \
	     (slot) || ((slot) = radix_tree_next_chunk((root), (iter), 0)); \
	     (slot)++)

#endif /* _LINUXKPI_LINUX_RADIX_TREE_H */
