/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/swap.h> shim (MPL-2.0, original work).
 *
 * Swap / memory-reclaim surface. amdgpu_ttm.c includes it for the global page
 * accounting it consults when sizing the TTM pools (si_meminfo / total RAM).
 * The reclaim machinery itself is out of the bring-up subset — TTM uses the GTT
 * size the host reports. Backed by ath_linuxkpi at M4. License boundary
 * (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_SWAP_H
#define _LINUXKPI_LINUX_SWAP_H

#include <linux/types.h>
#include <linux/mm.h>   /* struct sysinfo + si_meminfo */

/* total usable pages in the system (TTM caps its pools at a fraction of this). */
long si_mem_available(void);
unsigned long totalram_pages(void);

/* page-cache shrink hint — no-op for the bring-up daemon. */
static inline int add_to_swap(struct page *page) { (void)page; return 0; }

#endif /* _LINUXKPI_LINUX_SWAP_H */
