/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/list_sort.h> shim (MPL-2.0, original work).
 *
 * Stable merge sort of a struct list_head, with a caller-supplied comparator.
 * amdgpu uses it (RAS error records, BO eviction order). Backed by raeen_linuxkpi
 * at M4 (a list-aware sibling of its lib/sort.c heapsort). License boundary
 * (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_LIST_SORT_H
#define _LINUXKPI_LINUX_LIST_SORT_H

#include <linux/types.h>

struct list_head;

void list_sort(void *priv, struct list_head *head,
	       int (*cmp)(void *priv, const struct list_head *a, const struct list_head *b));

#endif /* _LINUXKPI_LINUX_LIST_SORT_H */
