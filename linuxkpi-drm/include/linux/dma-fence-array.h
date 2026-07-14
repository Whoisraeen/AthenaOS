/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/dma-fence-array.h> shim (MPL-2.0, original work).
 *
 * A composite fence that signals when an array of child fences all signal.
 * amdgpu uses it in the VM/sync paths. The type must be laid out for the
 * container_of accessors; the ops are backed by raeen_linuxkpi's fence facade at
 * M4. License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_DMA_FENCE_ARRAY_H
#define _LINUXKPI_LINUX_DMA_FENCE_ARRAY_H

#include <linux/dma-fence.h>
#include <linux/irq_work.h>

struct dma_fence_array_cb {
	struct dma_fence_cb cb;
	struct dma_fence_array *array;
};

struct dma_fence_array {
	struct dma_fence base;
	spinlock_t lock;
	unsigned int num_fences;
	unsigned int num_pending; /* atomic in the kernel; plain here (single pump) */
	struct dma_fence **fences;
	struct irq_work work;
};

extern const struct dma_fence_ops dma_fence_array_ops;

static inline bool dma_fence_is_array(struct dma_fence *fence)
{
	return fence->ops == &dma_fence_array_ops;
}

static inline struct dma_fence_array *to_dma_fence_array(struct dma_fence *fence)
{
	if (!fence || !dma_fence_is_array(fence))
		return (void *)0;
	return (struct dma_fence_array *)fence;
}

/* construction/iteration — backed by raeen_linuxkpi (M4) */
struct dma_fence_array *dma_fence_array_create(int num_fences, struct dma_fence **fences,
					       u64 context, unsigned int seqno, bool signal_on_any);
struct dma_fence *dma_fence_array_first(struct dma_fence *head);
struct dma_fence *dma_fence_array_next(struct dma_fence *head,
				       unsigned int index);

#endif /* _LINUXKPI_LINUX_DMA_FENCE_ARRAY_H */
