/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/gfp.h> shim (MPL-2.0, original work).
 *
 * The get-free-pages allocation flags (GFP_ and __GFP_ families) + the page
 * allocator. In
 * this shim the GFP flag vocabulary is defined alongside the slab allocator
 * (slab.h) and the page-alloc entry points live in mm.h; gfp.h re-exports both so
 * a driver header that includes only <linux/gfp.h> (e.g. drm_managed.h) gets the
 * flags + alloc_pages. License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_GFP_H
#define _LINUXKPI_LINUX_GFP_H

#include <linux/types.h>
#include <linux/slab.h>   /* GFP_KERNEL / __GFP_* flags + gfp_t */
#include <linux/mm.h>     /* alloc_pages / __get_free_pages / free_pages */

#endif /* _LINUXKPI_LINUX_GFP_H */
