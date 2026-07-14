/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/dma-fence-chain.h> shim (MPL-2.0, original work).
 *
 * A linked chain of fences with monotonically increasing seqnos (the timeline
 * syncobj primitive). amdgpu uses it in the sync/syncobj paths. Type laid out for
 * the accessors; ops backed by raeen_linuxkpi at M4. License boundary: API surface.
 */
#ifndef _LINUXKPI_LINUX_DMA_FENCE_CHAIN_H
#define _LINUXKPI_LINUX_DMA_FENCE_CHAIN_H

#include <linux/dma-fence.h>
#include <linux/irq_work.h>
#include <linux/rcupdate.h>

struct dma_fence_chain {
	struct dma_fence base;
	struct dma_fence *prev;
	u64 prev_seqno;
	struct dma_fence *fence;
	union {
		struct dma_fence_cb cb;
		struct irq_work work;
	};
	spinlock_t lock;
};

extern const struct dma_fence_ops dma_fence_chain_ops;

static inline struct dma_fence_chain *to_dma_fence_chain(struct dma_fence *fence)
{
	if (!fence || fence->ops != &dma_fence_chain_ops)
		return (void *)0;
	return (struct dma_fence_chain *)fence;
}

/* the chain node if `fence` is one (used to peel a syncobj timeline point). */
static inline struct dma_fence *dma_fence_chain_contained(struct dma_fence *fence)
{
	struct dma_fence_chain *chain = to_dma_fence_chain(fence);
	return chain ? chain->fence : fence;
}

/* iteration helpers (dma_fence_chain_for_each) — backed by raeen_linuxkpi (M4) */
struct dma_fence *dma_fence_chain_walk(struct dma_fence *fence);
int  dma_fence_chain_find_seqno(struct dma_fence **pfence, uint64_t seqno);
void dma_fence_chain_init(struct dma_fence_chain *chain, struct dma_fence *prev,
			  struct dma_fence *fence, uint64_t seqno);
struct dma_fence_chain *dma_fence_chain_alloc(void);
void dma_fence_chain_free(struct dma_fence_chain *chain);

#define dma_fence_chain_for_each(iter, head) \
	for (iter = dma_fence_get(head); iter; iter = dma_fence_chain_walk(iter))

#endif /* _LINUXKPI_LINUX_DMA_FENCE_CHAIN_H */
