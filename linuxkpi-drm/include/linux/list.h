/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/list.h> shim (MPL-2.0, original work).
 *
 * The kernel's intrusive doubly-linked list (and hlist). amdgpu/DRM thread nearly
 * every object onto one of these. These are REAL implementations — pure pointer
 * surgery, the only meaning the API can carry — not stubs. `struct list_head` /
 * `hlist_*` live in <linux/types.h>. License boundary (../../README.md): the list
 * API surface, no GPL source (the algorithms are the textbook circular-list ones
 * the names dictate).
 */
#ifndef _LINUXKPI_LINUX_LIST_H
#define _LINUXKPI_LINUX_LIST_H

#include <linux/types.h>
#include <stddef.h>

#ifndef container_of
#define container_of(ptr, type, member) \
	((type *)((char *)(ptr) - offsetof(type, member)))
#endif

#define LIST_HEAD_INIT(name) { &(name), &(name) }
#define LIST_HEAD(name) struct list_head name = LIST_HEAD_INIT(name)

static inline void INIT_LIST_HEAD(struct list_head *list) { list->next = list; list->prev = list; }

static inline void __list_add(struct list_head *new_, struct list_head *prev, struct list_head *next)
{
	next->prev = new_;
	new_->next = next;
	new_->prev = prev;
	prev->next = new_;
}
static inline void list_add(struct list_head *new_, struct list_head *head)      { __list_add(new_, head, head->next); }
static inline void list_add_tail(struct list_head *new_, struct list_head *head) { __list_add(new_, head->prev, head); }

static inline void __list_del(struct list_head *prev, struct list_head *next) { next->prev = prev; prev->next = next; }
static inline void __list_del_entry(struct list_head *entry) { __list_del(entry->prev, entry->next); }
static inline void list_del(struct list_head *entry) { __list_del(entry->prev, entry->next); entry->next = entry->prev = (void *)0; }
static inline void list_del_init(struct list_head *entry) { __list_del(entry->prev, entry->next); INIT_LIST_HEAD(entry); }

static inline int  list_empty(const struct list_head *head)       { return head->next == head; }
static inline int  list_is_last(const struct list_head *list, const struct list_head *head) { return list->next == head; }
static inline int  list_is_singular(const struct list_head *head) { return !list_empty(head) && (head->next == head->prev); }

static inline void list_move(struct list_head *list, struct list_head *head)      { __list_del(list->prev, list->next); list_add(list, head); }
static inline void list_move_tail(struct list_head *list, struct list_head *head) { __list_del(list->prev, list->next); list_add_tail(list, head); }
/* rotate @head so @list becomes the new front (kernel list_rotate_to_front). */
static inline void list_rotate_to_front(struct list_head *list, struct list_head *head) {
	struct list_head *new_head = list->next;
	list_del(head);
	__list_add(head, list, new_head);
}

static inline void list_replace(struct list_head *old, struct list_head *new_)
{
	new_->next = old->next;
	new_->next->prev = new_;
	new_->prev = old->prev;
	new_->prev->next = new_;
}
static inline void list_replace_init(struct list_head *old, struct list_head *new_) { list_replace(old, new_); INIT_LIST_HEAD(old); }

static inline void __list_splice(const struct list_head *list, struct list_head *prev, struct list_head *next)
{
	struct list_head *first = list->next, *last = list->prev;
	first->prev = prev; prev->next = first;
	last->next = next;  next->prev = last;
}
static inline void list_splice(const struct list_head *list, struct list_head *head)      { if (!list_empty(list)) __list_splice(list, head, head->next); }
static inline void list_splice_tail(const struct list_head *list, struct list_head *head) { if (!list_empty(list)) __list_splice(list, head->prev, head); }
static inline void list_splice_init(struct list_head *list, struct list_head *head)       { if (!list_empty(list)) { __list_splice(list, head, head->next); INIT_LIST_HEAD(list); } }
static inline void list_splice_tail_init(struct list_head *list, struct list_head *head)  { if (!list_empty(list)) { __list_splice(list, head->prev, head); INIT_LIST_HEAD(list); } }

#define list_entry(ptr, type, member)        container_of(ptr, type, member)
#define list_first_entry(ptr, type, member)  list_entry((ptr)->next, type, member)
#define list_last_entry(ptr, type, member)   list_entry((ptr)->prev, type, member)
#define list_first_entry_or_null(ptr, type, member) \
	(list_empty(ptr) ? (type *)0 : list_first_entry(ptr, type, member))
#define list_next_entry(pos, member)         list_entry((pos)->member.next, __typeof__(*(pos)), member)
#define list_prev_entry(pos, member)         list_entry((pos)->member.prev, __typeof__(*(pos)), member)
#define list_entry_is_head(pos, head, member) (&(pos)->member == (head))

#define list_for_each(pos, head) \
	for ((pos) = (head)->next; (pos) != (head); (pos) = (pos)->next)
#define list_for_each_prev(pos, head) \
	for ((pos) = (head)->prev; (pos) != (head); (pos) = (pos)->prev)
#define list_for_each_safe(pos, n, head) \
	for ((pos) = (head)->next, (n) = (pos)->next; (pos) != (head); (pos) = (n), (n) = (pos)->next)
#define list_for_each_prev_safe(pos, n, head) \
	for ((pos) = (head)->prev, (n) = (pos)->prev; (pos) != (head); (pos) = (n), (n) = (pos)->prev)
#define list_for_each_entry(pos, head, member) \
	for ((pos) = list_first_entry(head, __typeof__(*(pos)), member); \
	     !list_entry_is_head(pos, head, member); \
	     (pos) = list_next_entry(pos, member))
#define list_for_each_entry_safe(pos, n, head, member) \
	for ((pos) = list_first_entry(head, __typeof__(*(pos)), member), \
	     (n) = list_next_entry(pos, member); \
	     !list_entry_is_head(pos, head, member); \
	     (pos) = (n), (n) = list_next_entry(n, member))
#define list_for_each_entry_safe_reverse(pos, n, head, member) \
	for ((pos) = list_last_entry(head, __typeof__(*(pos)), member), \
	     (n) = list_prev_entry(pos, member); \
	     !list_entry_is_head(pos, head, member); \
	     (pos) = (n), (n) = list_prev_entry(n, member))
#define list_for_each_entry_reverse(pos, head, member) \
	for ((pos) = list_last_entry(head, __typeof__(*(pos)), member); \
	     !list_entry_is_head(pos, head, member); \
	     (pos) = list_prev_entry(pos, member))
#define list_for_each_entry_continue(pos, head, member) \
	for ((pos) = list_next_entry(pos, member); \
	     !list_entry_is_head(pos, head, member); \
	     (pos) = list_next_entry(pos, member))
#define list_for_each_entry_continue_reverse(pos, head, member) \
	for ((pos) = list_prev_entry(pos, member); \
	     !list_entry_is_head(pos, head, member); \
	     (pos) = list_prev_entry(pos, member))
#define list_for_each_entry_from(pos, head, member) \
	for (; !list_entry_is_head(pos, head, member); (pos) = list_next_entry(pos, member))

/* ---- hlist ---- */
#define HLIST_HEAD_INIT { .first = (void *)0 }
static inline void INIT_HLIST_NODE(struct hlist_node *h) { h->next = (void *)0; h->pprev = (void *)0; }
static inline int  hlist_unhashed(const struct hlist_node *h) { return !h->pprev; }
static inline int  hlist_empty(const struct hlist_head *h)    { return !h->first; }
static inline void hlist_add_head(struct hlist_node *n, struct hlist_head *h)
{
	struct hlist_node *first = h->first;
	n->next = first;
	if (first)
		first->pprev = &n->next;
	h->first = n;
	n->pprev = &h->first;
}
static inline void hlist_del(struct hlist_node *n)
{
	struct hlist_node *next = n->next, **pprev = n->pprev;
	if (pprev) {
		*pprev = next;
		if (next)
			next->pprev = pprev;
	}
}
static inline void hlist_del_init(struct hlist_node *n) { if (!hlist_unhashed(n)) { hlist_del(n); INIT_HLIST_NODE(n); } }
/* move the whole chain from @old to @new (amdgpu_sync splices its fence buckets). */
static inline void hlist_move_list(struct hlist_head *old, struct hlist_head *new_)
{
	new_->first = old->first;
	if (new_->first)
		new_->first->pprev = &new_->first;
	old->first = (struct hlist_node *)0;
}

#define hlist_entry(ptr, type, member) container_of(ptr, type, member)
#define hlist_entry_safe(ptr, type, member) \
	({ __typeof__(ptr) ____ptr = (ptr); ____ptr ? hlist_entry(____ptr, type, member) : (type *)0; })
#define hlist_for_each_entry(pos, head, member) \
	for ((pos) = hlist_entry_safe((head)->first, __typeof__(*(pos)), member); \
	     (pos); \
	     (pos) = hlist_entry_safe((pos)->member.next, __typeof__(*(pos)), member))
/* deletion-safe hlist walk: `n` (struct hlist_node *) holds the next link. */
#define hlist_for_each_entry_safe(pos, n, head, member) \
	for ((pos) = hlist_entry_safe((head)->first, __typeof__(*(pos)), member); \
	     (pos) && ({ (n) = (pos)->member.next; 1; }); \
	     (pos) = hlist_entry_safe(n, __typeof__(*(pos)), member))

#endif /* _LINUXKPI_LINUX_LIST_H */
