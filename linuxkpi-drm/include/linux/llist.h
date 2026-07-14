/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/llist.h> shim (MPL-2.0, original work).
 *
 * Lock-less singly-linked list (atomic push / batch-drain). amdgpu/DRM use it for
 * deferred-free and fence-signal queues touched from multiple threads. REAL
 * lockless implementation over the atomic builtins (cmpxchg/xchg) — the genuine
 * semantics, not a fake. License boundary (../../README.md): the llist API
 * surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_LLIST_H
#define _LINUXKPI_LINUX_LLIST_H

#include <linux/types.h>
#include <linux/atomic.h>
#include <linux/list.h>   /* container_of */

struct llist_head { struct llist_node *first; };
struct llist_node { struct llist_node *next; };

#define LLIST_HEAD_INIT(name) { (void *)0 }
#define LLIST_HEAD(name) struct llist_head name = LLIST_HEAD_INIT(name)

static inline void init_llist_head(struct llist_head *head) { head->first = (void *)0; }
static inline bool llist_empty(const struct llist_head *head)
{
	return __atomic_load_n(&head->first, __ATOMIC_ACQUIRE) == (void *)0;
}

/* push one (or a pre-linked new_first..new_last batch) atomically. */
static inline bool llist_add_batch(struct llist_node *new_first, struct llist_node *new_last,
				   struct llist_head *head)
{
	struct llist_node *first = __atomic_load_n(&head->first, __ATOMIC_RELAXED);
	do {
		new_last->next = first;
	} while (!__atomic_compare_exchange_n(&head->first, &first, new_first, false,
					      __ATOMIC_RELEASE, __ATOMIC_RELAXED));
	return first == (void *)0;
}
static inline bool llist_add(struct llist_node *new_, struct llist_head *head)
{
	return llist_add_batch(new_, new_, head);
}

/* atomically take the whole list. */
static inline struct llist_node *llist_del_all(struct llist_head *head)
{
	return __atomic_exchange_n(&head->first, (void *)0, __ATOMIC_ACQUIRE);
}
/* atomically pop the head. */
static inline struct llist_node *llist_del_first(struct llist_head *head)
{
	struct llist_node *entry = __atomic_load_n(&head->first, __ATOMIC_ACQUIRE);
	struct llist_node *next;
	while (entry) {
		next = entry->next;
		if (__atomic_compare_exchange_n(&head->first, &entry, next, false,
						__ATOMIC_ACQUIRE, __ATOMIC_ACQUIRE))
			return entry;
		/* entry reloaded with the current head on failure; retry */
	}
	return (void *)0;
}

#define llist_entry(ptr, type, member) container_of(ptr, type, member)
#define llist_for_each(pos, node) \
	for ((pos) = (node); (pos); (pos) = (pos)->next)
#define llist_for_each_safe(pos, n, node) \
	for ((pos) = (node); (pos) && ((n) = (pos)->next, true); (pos) = (n))
#define llist_for_each_entry(pos, node, member) \
	for ((pos) = llist_entry((node), __typeof__(*(pos)), member); \
	     &(pos)->member != (void *)0; \
	     (pos) = llist_entry((pos)->member.next, __typeof__(*(pos)), member))
#define llist_for_each_entry_safe(pos, n, node, member) \
	for ((pos) = llist_entry((node), __typeof__(*(pos)), member); \
	     &(pos)->member != (void *)0 && \
	     ((n) = llist_entry((pos)->member.next, __typeof__(*(pos)), member), true); \
	     (pos) = (n))

#endif /* _LINUXKPI_LINUX_LLIST_H */
