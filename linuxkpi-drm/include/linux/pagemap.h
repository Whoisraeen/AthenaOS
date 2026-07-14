/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/pagemap.h> shim (MPL-2.0, original work).
 *
 * Page-cache / address-space helpers. TTM's ttm_tt pulls it for shmem-backed BO
 * pages — TTM is out of the MES bring-up subset (SCOPE.md), so this provides the
 * minimal mapping-flag + page-size surface the type graph needs; the page-cache
 * ops are backed by ath_linuxkpi at M4 if TTM is brought into scope. License
 * boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_PAGEMAP_H
#define _LINUXKPI_LINUX_PAGEMAP_H

#include <linux/types.h>
#include <linux/mm.h>

struct address_space;
struct page;
struct file;

static inline unsigned long page_size_helper(void) { return PAGE_SIZE; }

/* page-cache ops — backed by ath_linuxkpi (M4) if TTM is brought into scope */
void mapping_set_gfp_mask(struct address_space *m, gfp_t mask);

#endif /* _LINUXKPI_LINUX_PAGEMAP_H */
