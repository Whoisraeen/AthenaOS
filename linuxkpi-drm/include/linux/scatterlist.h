/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/scatterlist.h> shim (MPL-2.0, original work).
 *
 * Scatter-gather lists — amdgpu/TTM/PRIME describe BO page sets with these. The
 * accessors (sg_page/sg_dma_address/sg_next/for_each_sg) are pure field/pointer
 * ops, inlined. The table-alloc + dma_map_sg ops are backed by ath_linuxkpi's
 * existing scatterlist facade at M4 (it maps page==virtual-base identity DMA). The
 * struct layout matches the upstream ABI the facade expects (page_link low bits
 * tag chain/last). License boundary (../../README.md): API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_SCATTERLIST_H
#define _LINUXKPI_LINUX_SCATTERLIST_H

#include <linux/types.h>
#include <linux/mm.h>

struct scatterlist {
	unsigned long page_link;   /* struct page* with low bits: 0x1 chain, 0x2 last */
	unsigned int  offset;
	unsigned int  length;
	dma_addr_t    dma_address;
	unsigned int  dma_length;
};

struct sg_table {
	struct scatterlist *sgl;
	unsigned int        nents;
	unsigned int        orig_nents;
};

#define SG_CHAIN 0x1UL
#define SG_END   0x2UL

static inline struct page *sg_page(struct scatterlist *sg) { return (struct page *)(sg->page_link & ~0x3UL); }
static inline void sg_assign_page(struct scatterlist *sg, struct page *page)
{ sg->page_link = (sg->page_link & 0x3UL) | (unsigned long)page; }
static inline void sg_set_page(struct scatterlist *sg, struct page *page, unsigned int len, unsigned int offset)
{ sg_assign_page(sg, page); sg->offset = offset; sg->length = len; }
static inline bool sg_is_chain(struct scatterlist *sg) { return (sg->page_link & SG_CHAIN) != 0; }
static inline bool sg_is_last(struct scatterlist *sg)  { return (sg->page_link & SG_END) != 0; }
static inline struct scatterlist *sg_next(struct scatterlist *sg)
{ if (sg_is_last(sg)) return (struct scatterlist *)0; return sg + 1; }

#define sg_dma_address(sg) ((sg)->dma_address)
#define sg_dma_len(sg)     ((sg)->dma_length)

#define for_each_sg(sglist, sg, nr, __i) \
	for ((__i) = 0, (sg) = (sglist); (__i) < (nr); (__i)++, (sg) = sg_next(sg))
#define for_each_sgtable_dma_sg(sgt, sg, i) \
	for_each_sg((sgt)->sgl, sg, (sgt)->nents, i)
#define for_each_sgtable_sg(sgt, sg, i) \
	for_each_sg((sgt)->sgl, sg, (sgt)->orig_nents, i)

/* Page-wise iterator used by DRM PRIME to flatten a DMA-mapped sg_table. */
struct sg_page_iter {
	struct scatterlist *sg;
	unsigned int sg_pgoffset;
	unsigned int __nents;
	int __pg_advance;
};

struct sg_dma_page_iter {
	struct sg_page_iter base;
};

static inline void __sg_page_iter_start(struct sg_page_iter *iter,
					struct scatterlist *sglist,
					unsigned int nents,
					unsigned long pgoffset)
{
	iter->__pg_advance = 0;
	iter->__nents = nents;
	iter->sg = sglist;
	iter->sg_pgoffset = (unsigned int)pgoffset;
}

static inline unsigned int __sg_dma_page_count(struct scatterlist *sg)
{
	return (unsigned int)PAGE_ALIGN((unsigned long)sg->offset + sg_dma_len(sg)) >>
	       PAGE_SHIFT;
}

static inline bool __sg_page_iter_dma_next(struct sg_dma_page_iter *dma_iter)
{
	struct sg_page_iter *iter = &dma_iter->base;

	if (!iter->__nents || !iter->sg)
		return false;

	iter->sg_pgoffset += iter->__pg_advance;
	iter->__pg_advance = 1;
	while (iter->sg_pgoffset >= __sg_dma_page_count(iter->sg)) {
		iter->sg_pgoffset -= __sg_dma_page_count(iter->sg);
		iter->sg = sg_next(iter->sg);
		if (!--iter->__nents || !iter->sg)
			return false;
	}
	return true;
}

static inline unsigned int __sg_page_count(struct scatterlist *sg)
{
	return (unsigned int)PAGE_ALIGN((unsigned long)sg->offset + sg->length) >>
	       PAGE_SHIFT;
}

static inline bool __sg_page_iter_next(struct sg_page_iter *iter)
{
	if (!iter->__nents || !iter->sg)
		return false;

	iter->sg_pgoffset += iter->__pg_advance;
	iter->__pg_advance = 1;
	while (iter->sg_pgoffset >= __sg_page_count(iter->sg)) {
		iter->sg_pgoffset -= __sg_page_count(iter->sg);
		iter->sg = sg_next(iter->sg);
		if (!--iter->__nents || !iter->sg)
			return false;
	}
	return true;
}

static inline struct page *sg_page_iter_page(struct sg_page_iter *iter)
{
	return sg_page(iter->sg) + iter->sg_pgoffset;
}

static inline dma_addr_t
sg_page_iter_dma_address(struct sg_dma_page_iter *dma_iter)
{
	return sg_dma_address(dma_iter->base.sg) +
	       ((dma_addr_t)dma_iter->base.sg_pgoffset << PAGE_SHIFT);
}

#define for_each_sg_dma_page(sglist, dma_iter, dma_nents, pgoffset) \
	for (__sg_page_iter_start(&(dma_iter)->base, (sglist), (dma_nents), \
				  (pgoffset)); \
	     __sg_page_iter_dma_next(dma_iter);)

#define for_each_sgtable_dma_page(sgt, dma_iter, pgoffset) \
	for_each_sg_dma_page((sgt)->sgl, dma_iter, (sgt)->nents, pgoffset)

#define for_each_sg_page(sglist, iter, nents, pgoffset) \
	for (__sg_page_iter_start((iter), (sglist), (nents), (pgoffset)); \
	     __sg_page_iter_next(iter);)

#define for_each_sgtable_page(sgt, iter, pgoffset) \
	for_each_sg_page((sgt)->sgl, iter, (sgt)->orig_nents, pgoffset)

/* table alloc + DMA map — backed by ath_linuxkpi's scatterlist facade (M4) */
int  sg_alloc_table(struct sg_table *table, unsigned int nents, gfp_t gfp);
void sg_free_table(struct sg_table *table);
void sg_init_table(struct scatterlist *sgl, unsigned int nents);
struct scatterlist *sg_alloc_table_from_pages(struct sg_table *sgt, struct page **pages,
		unsigned int n_pages, unsigned int offset, unsigned long size, gfp_t gfp);
int  dma_map_sg(struct device *dev, struct scatterlist *sg, int nents, int dir);
void dma_unmap_sg(struct device *dev, struct scatterlist *sg, int nents, int dir);

#endif /* _LINUXKPI_LINUX_SCATTERLIST_H */
