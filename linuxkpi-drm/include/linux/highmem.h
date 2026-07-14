/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/highmem.h> shim (MPL-2.0, original work).
 *
 * Temporary page mappings. On x86_64 there is no highmem — every page is in the
 * linear map — so kmap/kmap_local are just page_address() and the unmaps are
 * no-ops. TTM's pool/tt use these to zero and copy BO pages. REAL for the
 * bring-up page facade (page_address is exported by ath_linuxkpi). License
 * boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_HIGHMEM_H
#define _LINUXKPI_LINUX_HIGHMEM_H

#include <linux/mm.h>
#include <linux/string.h>
/* TTM's pool (the only highmem.h consumer here) also spells struct shrinker +
 * SHRINK_STOP and a DEFINE_RWSEM without including their headers, relying on
 * kernel transitivity; pull them so ttm_pool.c resolves. */
#include <linux/shrinker.h>
#include <linux/rwsem.h>

static inline void *kmap(struct page *page)              { return page_address(page); }
static inline void  kunmap(struct page *page)            { (void)page; }
static inline void *kmap_local_page(struct page *page)   { return page_address(page); }
static inline void  kunmap_local(const void *addr)       { (void)addr; }
static inline void *kmap_atomic(struct page *page)       { return page_address(page); }
static inline void  kunmap_atomic(const void *addr)      { (void)addr; }

static inline void clear_page_addr(void *a) { if (a) memset(a, 0, PAGE_SIZE); }
static inline void clear_highpage(struct page *page)     { clear_page_addr(page_address(page)); }
static inline void clear_user_highpage(struct page *page, unsigned long vaddr)
{ (void)vaddr; clear_highpage(page); }

static inline void memcpy_from_page(void *to, struct page *from, size_t off, size_t len)
{ char *a = page_address(from); if (a) memcpy(to, a + off, len); }
static inline void memcpy_to_page(struct page *to, size_t off, const void *from, size_t len)
{ char *a = page_address(to); if (a) memcpy(a + off, from, len); }

#endif /* _LINUXKPI_LINUX_HIGHMEM_H */
