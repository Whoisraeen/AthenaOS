/* SPDX-License-Identifier: MPL-2.0 */
/* Generic in-place sort API used by upstream DRM BO-list validation. */
#ifndef _LINUXKPI_LINUX_SORT_H
#define _LINUXKPI_LINUX_SORT_H

#include <linux/types.h>

void sort(void *base, size_t num, size_t size,
	  int (*cmp)(const void *, const void *),
	  void (*swap_func)(void *, void *, int));

#endif
