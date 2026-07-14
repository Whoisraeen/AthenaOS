/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/rbtree.h> shim (MPL-2.0, original work).
 *
 * Red-black tree. drm_mm (the GPU address-space allocator) and amdgpu's VM keep
 * their interval trees here. The node/root accessors + rb_link_node are pure
 * pointer setup (inlined); the BALANCING algorithm (insert_color/erase/iterate)
 * is backed by raeen_linuxkpi at M4 — a fake that skipped rebalancing would
 * silently corrupt the allocator's ordering (SCOPE.md rule 9). The node layout
 * matches the upstream ABI (parent+color packed in __rb_parent_color). License
 * boundary (../../README.md): API surface only.
 */
#ifndef _LINUXKPI_LINUX_RBTREE_H
#define _LINUXKPI_LINUX_RBTREE_H

#include <linux/types.h>
#include <linux/list.h>   /* container_of */

struct rb_node {
	unsigned long   __rb_parent_color;
	struct rb_node *rb_right;
	struct rb_node *rb_left;
} __attribute__((aligned(sizeof(long))));

struct rb_root { struct rb_node *rb_node; };
struct rb_root_cached {
	struct rb_root  rb_root;
	struct rb_node *rb_leftmost;
};

#define RB_ROOT          (struct rb_root) { (struct rb_node *)0 }
#define RB_ROOT_CACHED   (struct rb_root_cached) { { (struct rb_node *)0 }, (struct rb_node *)0 }
#define rb_entry(ptr, type, member) container_of(ptr, type, member)
#define rb_entry_safe(ptr, type, member) \
	({ __typeof__(ptr) ____ptr = (ptr); ____ptr ? rb_entry(____ptr, type, member) : (type *)0; })

/* parent accessor (colour packed in the low bits) — used by the interval-tree
 * iterators and any augmented walker. */
#define rb_parent(r)   ((struct rb_node *)((r)->__rb_parent_color & ~3UL))

#define RB_EMPTY_ROOT(root)  ((root)->rb_node == (struct rb_node *)0)
#define RB_EMPTY_NODE(node)  ((node)->__rb_parent_color == (unsigned long)(node))
#define RB_CLEAR_NODE(node)  ((node)->__rb_parent_color = (unsigned long)(node))

static inline void rb_link_node(struct rb_node *node, struct rb_node *parent, struct rb_node **rb_link)
{
	node->__rb_parent_color = (unsigned long)parent;
	node->rb_left = node->rb_right = (struct rb_node *)0;
	*rb_link = node;
}

/* balancing + iteration — backed by raeen_linuxkpi (M4) */
void rb_insert_color(struct rb_node *node, struct rb_root *root);
void rb_erase(struct rb_node *node, struct rb_root *root);
struct rb_node *rb_next(const struct rb_node *node);
struct rb_node *rb_prev(const struct rb_node *node);
struct rb_node *rb_first(const struct rb_root *root);
struct rb_node *rb_last(const struct rb_root *root);
void rb_replace_node(struct rb_node *victim, struct rb_node *new_, struct rb_root *root);

void rb_insert_color_cached(struct rb_node *node, struct rb_root_cached *root, bool leftmost);
void rb_erase_cached(struct rb_node *node, struct rb_root_cached *root);
struct rb_node *rb_first_cached(const struct rb_root_cached *root);

/* rb_add_cached: comparator-driven insert (upstream <linux/rbtree.h> inline,
 * verbatim algorithm) — walks the tree with `less`, tracking whether the new
 * node became the leftmost, then defers to rb_link_node (pure pointer setup,
 * above) + rb_insert_color_cached (the real rebalance, backed by
 * raeen_linuxkpi). drm/scheduler's rq uses this to keep entities ordered by
 * priority/vruntime; no new Rust surface needed — this IS how upstream
 * implements it, not a reimplementation. */
static inline struct rb_node *
rb_add_cached(struct rb_node *node, struct rb_root_cached *tree,
	      bool (*less)(struct rb_node *, const struct rb_node *))
{
	struct rb_node **link = &tree->rb_root.rb_node;
	struct rb_node *parent = (struct rb_node *)0;
	bool leftmost = true;

	while (*link) {
		parent = *link;
		if (less(node, parent)) {
			link = &parent->rb_left;
		} else {
			link = &parent->rb_right;
			leftmost = false;
		}
	}

	rb_link_node(node, parent, link);
	rb_insert_color_cached(node, tree, leftmost);

	return leftmost ? node : (struct rb_node *)0;
}

/* rb_add: the non-cached comparator-driven insert (upstream <linux/rbtree.h>
 * inline, verbatim algorithm). drm_buddy keeps its per-order free-block trees
 * ordered by offset with this; same pattern as rb_add_cached — pure walk +
 * rb_link_node + the real rb_insert_color rebalance (backed by raeen_linuxkpi). */
static inline void
rb_add(struct rb_node *node, struct rb_root *tree,
       bool (*less)(struct rb_node *, const struct rb_node *))
{
	struct rb_node **link = &tree->rb_node;
	struct rb_node *parent = (struct rb_node *)0;

	while (*link) {
		parent = *link;
		if (less(node, parent))
			link = &parent->rb_left;
		else
			link = &parent->rb_right;
	}

	rb_link_node(node, parent, link);
	rb_insert_color(node, tree);
}

#define rbtree_postorder_for_each_entry_safe(pos, n, root, field) \
	for ((pos) = rb_entry_safe(rb_first(root), __typeof__(*(pos)), field); \
	     (pos) && ((n) = rb_entry_safe(rb_next(&(pos)->field), __typeof__(*(pos)), field), 1); \
	     (pos) = (n))

#endif /* _LINUXKPI_LINUX_RBTREE_H */
