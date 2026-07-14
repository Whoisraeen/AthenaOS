/* SPDX-License-Identifier: MPL-2.0 */
/* Container-fence iterator API used by timeline syncobjs. */
#ifndef _LINUXKPI_LINUX_DMA_FENCE_UNWRAP_H
#define _LINUXKPI_LINUX_DMA_FENCE_UNWRAP_H

struct dma_fence;

struct dma_fence_unwrap {
	struct dma_fence *chain;
	struct dma_fence *array;
	unsigned int index;
};

struct dma_fence *dma_fence_unwrap_first(struct dma_fence *head,
					 struct dma_fence_unwrap *cursor);
struct dma_fence *dma_fence_unwrap_next(struct dma_fence_unwrap *cursor);

#define dma_fence_unwrap_for_each(fence, cursor, head) \
	for ((fence) = dma_fence_unwrap_first((head), (cursor)); (fence); \
	     (fence) = dma_fence_unwrap_next(cursor))

struct dma_fence *__dma_fence_unwrap_merge(unsigned int num_fences,
					   struct dma_fence **fences,
					   struct dma_fence_unwrap *cursors);
int dma_fence_dedup_array(struct dma_fence **array, int num_fences);

#define dma_fence_unwrap_merge(...) \
	({ \
		struct dma_fence *__f[] = { __VA_ARGS__ }; \
		struct dma_fence_unwrap __c[sizeof(__f) / sizeof(__f[0])]; \
		__dma_fence_unwrap_merge(sizeof(__f) / sizeof(__f[0]), __f, __c); \
	})

#endif /* _LINUXKPI_LINUX_DMA_FENCE_UNWRAP_H */
