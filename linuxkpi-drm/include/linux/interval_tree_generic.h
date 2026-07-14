/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/interval_tree_generic.h> shim (MPL-2.0, original work).
 *
 * INTERVAL_TREE_DEFINE generates a type-specialised augmented-rbtree interval
 * tree: insert/remove keyed on [start;last], plus iter_first/iter_next that walk
 * every stored interval overlapping a query [start;last]. amdgpu's VM keeps its
 * BO mappings in one (amdgpu_vm.c). Built on the augmented-rbtree cores in
 * ath_linuxkpi (rbtree.rs) via <linux/rbtree_augmented.h>; the algorithm is the
 * well-known max-endpoint augmented interval tree, reimplemented as original MPL
 * work. A fake search would silently miss overlapping mappings (SCOPE.md rule 9).
 * License boundary (../../README.md): API surface + original macro implementation.
 */
#ifndef _LINUXKPI_LINUX_INTERVAL_TREE_GENERIC_H
#define _LINUXKPI_LINUX_INTERVAL_TREE_GENERIC_H

#include <linux/rbtree_augmented.h>

/*
 * Parameters:
 *   ITSTRUCT   the struct embedding the node + the interval fields
 *   ITRB       the struct rb_node member name
 *   ITTYPE     the endpoint scalar type
 *   ITSUBTREE  the per-node "max last in subtree" summary field
 *   ITSTART(n) the interval start accessor
 *   ITLAST(n)  the interval last (inclusive end) accessor
 *   ITSTATIC   storage class of the generated insert/remove/iter functions
 *   ITPREFIX   name prefix for the generated functions
 */
#define INTERVAL_TREE_DEFINE(ITSTRUCT, ITRB, ITTYPE, ITSUBTREE,		      \
			     ITSTART, ITLAST, ITSTATIC, ITPREFIX)	      \
									      \
/* the augment callbacks maintaining ITSUBTREE = max(ITLAST over the subtree).
 * The MAX macro ends in a `struct ... = {...}` declaration; terminate it. */	      \
RB_DECLARE_CALLBACKS_MAX(static, ITPREFIX ## _augment, ITSTRUCT, ITRB,	      \
			 ITTYPE, ITSUBTREE, ITLAST);			      \
									      \
ITSTATIC void ITPREFIX ## _insert(ITSTRUCT *node,			      \
				  struct rb_root_cached *root)		      \
{									      \
	struct rb_node **link = &root->rb_root.rb_node, *rb_parent = (void *)0;\
	ITTYPE start = ITSTART(node), last = ITLAST(node);		      \
	ITSTRUCT *parent;						      \
	bool leftmost = true;						      \
									      \
	while (*link) {							      \
		rb_parent = *link;					      \
		parent = rb_entry(rb_parent, ITSTRUCT, ITRB);		      \
		if (parent->ITSUBTREE < last)				      \
			parent->ITSUBTREE = last;			      \
		if (start < ITSTART(parent)) {				      \
			link = &parent->ITRB.rb_left;			      \
		} else {						      \
			link = &parent->ITRB.rb_right;			      \
			leftmost = false;				      \
		}							      \
	}								      \
									      \
	node->ITSUBTREE = last;						      \
	rb_link_node(&node->ITRB, rb_parent, link);			      \
	rb_insert_augmented_cached(&node->ITRB, root,			      \
				   leftmost, &ITPREFIX ## _augment);	      \
}									      \
									      \
ITSTATIC void ITPREFIX ## _remove(ITSTRUCT *node,			      \
				  struct rb_root_cached *root)		      \
{									      \
	rb_erase_augmented_cached(&node->ITRB, root, &ITPREFIX ## _augment);  \
}									      \
									      \
/* leftmost interval under `node` overlapping [start;last], or NULL */	      \
static ITSTRUCT *							      \
ITPREFIX ## _subtree_search(ITSTRUCT *node, ITTYPE start, ITTYPE last)	      \
{									      \
	while (1) {							      \
		if (node->ITRB.rb_left) {				      \
			ITSTRUCT *left = rb_entry(node->ITRB.rb_left,	      \
						  ITSTRUCT, ITRB);	      \
			if (start <= left->ITSUBTREE) {			      \
				node = left;				      \
				continue;				      \
			}						      \
		}							      \
		if (ITSTART(node) <= last) {				      \
			if (start <= ITLAST(node))			      \
				return node;				      \
			if (node->ITRB.rb_right) {			      \
				node = rb_entry(node->ITRB.rb_right,	      \
						ITSTRUCT, ITRB);	      \
				if (start <= node->ITSUBTREE)		      \
					continue;			      \
			}						      \
		}							      \
		return (ITSTRUCT *)0;					      \
	}								      \
}									      \
									      \
ITSTATIC ITSTRUCT *							      \
ITPREFIX ## _iter_first(struct rb_root_cached *root,			      \
			ITTYPE start, ITTYPE last)			      \
{									      \
	ITSTRUCT *node, *leftmost;					      \
									      \
	if (!root->rb_root.rb_node)					      \
		return (ITSTRUCT *)0;					      \
									      \
	leftmost = rb_entry(root->rb_leftmost, ITSTRUCT, ITRB);		      \
	if (ITLAST(leftmost) < start)					      \
		return (ITSTRUCT *)0;					      \
									      \
	node = rb_entry(root->rb_root.rb_node, ITSTRUCT, ITRB);		      \
	if (node->ITSUBTREE < start)					      \
		return (ITSTRUCT *)0;					      \
									      \
	return ITPREFIX ## _subtree_search(node, start, last);		      \
}									      \
									      \
ITSTATIC ITSTRUCT *							      \
ITPREFIX ## _iter_next(ITSTRUCT *node, ITTYPE start, ITTYPE last)	      \
{									      \
	struct rb_node *rb = node->ITRB.rb_right, *prev;		      \
									      \
	while (1) {							      \
		if (rb) {						      \
			ITSTRUCT *right = rb_entry(rb, ITSTRUCT, ITRB);	      \
			if (start <= right->ITSUBTREE) {		      \
				node = ITPREFIX ## _subtree_search(right,     \
							start, last);	      \
				if (node)				      \
					return node;			      \
			}						      \
		}							      \
		do {							      \
			rb = rb_parent(&node->ITRB);			      \
			if (!rb)					      \
				return (ITSTRUCT *)0;			      \
			prev = &node->ITRB;				      \
			node = rb_entry(rb, ITSTRUCT, ITRB);		      \
			rb = node->ITRB.rb_right;			      \
		} while (prev == rb);					      \
		if (last < ITSTART(node))				      \
			return (ITSTRUCT *)0;				      \
		else if (start <= ITLAST(node))				      \
			return node;					      \
	}								      \
}

#endif /* _LINUXKPI_LINUX_INTERVAL_TREE_GENERIC_H */
