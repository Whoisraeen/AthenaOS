/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/rbtree_augmented.h> shim (MPL-2.0, original work).
 *
 * Augmented red-black trees: each node carries a summary of its subtree (e.g. the
 * interval tree's max-endpoint), recomputed by callbacks invoked at every
 * rotation. The balancing cores (__rb_insert_augmented / __rb_erase_augmented /
 * __rb_erase_color) are the SAME exports raeen_linuxkpi's rbtree.rs provides for
 * the plain tree, taking the augment callbacks as function pointers; the public
 * struct-callback wrappers below are inline, exactly as the kernel factors them.
 *
 * RB_DECLARE_CALLBACKS_MAX generates the propagate/copy/rotate callbacks for a
 * "max of a field" augment (what interval_tree_generic.h builds on). License
 * boundary (../../README.md): API surface + the original macro implementation.
 */
#ifndef _LINUXKPI_LINUX_RBTREE_AUGMENTED_H
#define _LINUXKPI_LINUX_RBTREE_AUGMENTED_H

#include <linux/rbtree.h>
#include <linux/types.h>

struct rb_augment_callbacks {
	void (*propagate)(struct rb_node *node, struct rb_node *stop);
	void (*copy)(struct rb_node *old, struct rb_node *new_);
	void (*rotate)(struct rb_node *old, struct rb_node *new_);
};

/* balancing cores — implemented in raeen_linuxkpi/src/rbtree.rs (M4). The augment
 * callbacks are passed as bare function pointers (the wrappers below unpack the
 * struct). __rb_erase_augmented returns the node to start colour-fixup from. */
void __rb_insert_augmented(struct rb_node *node, struct rb_root *root,
			   void (*augment_rotate)(struct rb_node *old, struct rb_node *new_));
struct rb_node *__rb_erase_augmented(struct rb_node *node, struct rb_root *root,
			   void (*propagate)(struct rb_node *node, struct rb_node *stop),
			   void (*copy)(struct rb_node *old, struct rb_node *new_));
void __rb_erase_color(struct rb_node *parent, struct rb_root *root,
		      void (*augment_rotate)(struct rb_node *old, struct rb_node *new_));

static inline void
rb_insert_augmented(struct rb_node *node, struct rb_root *root,
		    const struct rb_augment_callbacks *augment)
{
	__rb_insert_augmented(node, root, augment->rotate);
}

static inline void
rb_insert_augmented_cached(struct rb_node *node, struct rb_root_cached *root,
			   bool newleft, const struct rb_augment_callbacks *augment)
{
	if (newleft)
		root->rb_leftmost = node;
	__rb_insert_augmented(node, &root->rb_root, augment->rotate);
}

static inline void
rb_erase_augmented(struct rb_node *node, struct rb_root *root,
		   const struct rb_augment_callbacks *augment)
{
	struct rb_node *rebalance =
		__rb_erase_augmented(node, root, augment->propagate, augment->copy);
	if (rebalance)
		__rb_erase_color(rebalance, root, augment->rotate);
}

static inline void
rb_erase_augmented_cached(struct rb_node *node, struct rb_root_cached *root,
			  const struct rb_augment_callbacks *augment)
{
	if (root->rb_leftmost == node)
		root->rb_leftmost = rb_next(node);
	rb_erase_augmented(node, &root->rb_root, augment);
}

/*
 * Template for declaring augmented rbtree callbacks where the stored summary is
 * the MAXIMUM of `rbcompute(node)` over the subtree (the interval-tree shape).
 *   rbstatic   storage class of the generated `struct rb_augment_callbacks`
 *   rbname     name of that callbacks instance (and the prefix of its helpers)
 *   rbstruct   the embedding struct type
 *   rbfield    the struct rb_node member name
 *   rbtype     type of the summary field
 *   rbaugmented the summary field name
 *   rbcompute  macro/fn computing this node's own contribution: rbcompute(node)
 */
#define RB_DECLARE_CALLBACKS_MAX(rbstatic, rbname, rbstruct, rbfield,	      \
				 rbtype, rbaugmented, rbcompute)	      \
static inline bool rbname ## _compute_max(rbstruct *node, bool exit)	      \
{									      \
	rbstruct *child;						      \
	rbtype max = rbcompute(node);					      \
	if (node->rbfield.rb_left) {					      \
		child = rb_entry(node->rbfield.rb_left, rbstruct, rbfield);    \
		if (child->rbaugmented > max)				      \
			max = child->rbaugmented;			      \
	}								      \
	if (node->rbfield.rb_right) {					      \
		child = rb_entry(node->rbfield.rb_right, rbstruct, rbfield);   \
		if (child->rbaugmented > max)				      \
			max = child->rbaugmented;			      \
	}								      \
	if (exit && node->rbaugmented == max)				      \
		return true;						      \
	node->rbaugmented = max;					      \
	return false;							      \
}									      \
static inline void							      \
rbname ## _propagate(struct rb_node *rb, struct rb_node *stop)		      \
{									      \
	while (rb != stop) {						      \
		rbstruct *node = rb_entry(rb, rbstruct, rbfield);	      \
		if (rbname ## _compute_max(node, true))			      \
			break;						      \
		rb = rb_parent(&node->rbfield);				      \
	}								      \
}									      \
static inline void							      \
rbname ## _copy(struct rb_node *rb_old, struct rb_node *rb_new)		      \
{									      \
	rbstruct *old = rb_entry(rb_old, rbstruct, rbfield);		      \
	rbstruct *new_ = rb_entry(rb_new, rbstruct, rbfield);		      \
	new_->rbaugmented = old->rbaugmented;				      \
}									      \
static void								      \
rbname ## _rotate(struct rb_node *rb_old, struct rb_node *rb_new)	      \
{									      \
	rbstruct *old = rb_entry(rb_old, rbstruct, rbfield);		      \
	rbstruct *new_ = rb_entry(rb_new, rbstruct, rbfield);		      \
	new_->rbaugmented = old->rbaugmented;				      \
	rbname ## _compute_max(old, false);				      \
}									      \
rbstatic const struct rb_augment_callbacks rbname = {			      \
	.propagate = rbname ## _propagate,				      \
	.copy = rbname ## _copy,					      \
	.rotate = rbname ## _rotate,					      \
};

#endif /* _LINUXKPI_LINUX_RBTREE_AUGMENTED_H */
