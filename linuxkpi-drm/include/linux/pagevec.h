/* SPDX-License-Identifier: MPL-2.0 */
/* Small folio batch facade used by DRM GEM's shmem helpers. RaeenOS' current
 * page model is single-page folios; unsupported shmem creation fails closed. */
#ifndef _LINUXKPI_LINUX_PAGEVEC_H
#define _LINUXKPI_LINUX_PAGEVEC_H

#include <linux/mm.h>

#define PAGEVEC_SIZE 15

struct folio {
	struct page page;
};

struct folio_batch {
	unsigned int nr;
	struct folio *folios[PAGEVEC_SIZE];
};

static inline void folio_batch_init(struct folio_batch *batch)
{
	batch->nr = 0;
}

static inline unsigned int folio_batch_count(const struct folio_batch *batch)
{
	return batch->nr;
}

static inline bool folio_batch_add(struct folio_batch *batch, struct folio *folio)
{
	if (batch->nr >= PAGEVEC_SIZE)
		return false;
	batch->folios[batch->nr++] = folio;
	return batch->nr < PAGEVEC_SIZE;
}

static inline struct folio *page_folio(struct page *page)
{
	return (struct folio *)page;
}

static inline unsigned long folio_nr_pages(const struct folio *folio)
{
	(void)folio;
	return 1;
}

static inline struct page *folio_file_page(struct folio *folio,
					   unsigned long index)
{
	(void)index;
	return &folio->page;
}

static inline unsigned long folio_pfn(const struct folio *folio)
{
	return page_to_pfn(&folio->page);
}

static inline void folio_mark_dirty(struct folio *folio)
{
	folio->page.flags |= 1UL;
}

static inline void folio_mark_accessed(struct folio *folio)
{
	folio->page.flags |= 2UL;
}

static inline void check_move_unevictable_folios(struct folio_batch *batch)
{
	(void)batch;
}

static inline void __folio_batch_release(struct folio_batch *batch)
{
	unsigned int i;
	for (i = 0; i < batch->nr; i++)
		put_page(&batch->folios[i]->page);
	batch->nr = 0;
}

#endif
