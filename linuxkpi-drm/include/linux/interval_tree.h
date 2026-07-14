/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/interval_tree.h> shim (MPL-2.0, original work).
 *
 * Augmented red-black interval tree. amdgpu's VM tracks mapped address ranges in
 * one. The node embeds an rb_node (real layout); the insert/remove/iterate ops
 * are backed by raeen_linuxkpi at M4 (a fake tree would lose range overlaps —
 * SCOPE.md rule 9). License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_INTERVAL_TREE_H
#define _LINUXKPI_LINUX_INTERVAL_TREE_H

#include <linux/types.h>
#include <linux/rbtree.h>

struct interval_tree_node {
	struct rb_node rb;
	unsigned long  start; /* first address covered */
	unsigned long  last;  /* last address covered (inclusive) */
};

void interval_tree_insert(struct interval_tree_node *node, struct rb_root_cached *root);
void interval_tree_remove(struct interval_tree_node *node, struct rb_root_cached *root);
struct interval_tree_node *interval_tree_iter_first(struct rb_root_cached *root,
						    unsigned long start, unsigned long last);
struct interval_tree_node *interval_tree_iter_next(struct interval_tree_node *node,
						   unsigned long start, unsigned long last);

#endif /* _LINUXKPI_LINUX_INTERVAL_TREE_H */
