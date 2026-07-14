/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/dma-fence.h> shim (MPL-2.0, original work).
 *
 * The kernel's cross-driver completion primitive. amdgpu's ENTIRE submit path is
 * fences: every ring submission returns one, the DRM GPU scheduler is built on
 * them, and MES/IB completion is signalled through them. The struct layout uses
 * the stable upstream member names amdgpu reads (ops/seqno/context/flags/error/
 * refcount/lock/timestamp/cb_list).
 *
 * The SIGNALLING MACHINERY (init/signal/add_callback/wait) has real ordering and
 * wakeup obligations — a fake "always signalled" or a no-op wait would silently
 * break every GPU sync — so those are declaration-only, backed by raeen_linuxkpi's
 * fence facade at M4 (SCOPE.md rule 9). Only the genuinely-pure accessors
 * (set_error, seqno comparison) are inlined here.
 *
 * License boundary (../../README.md): the dma_fence API surface, not GPL source.
 */
#ifndef _LINUXKPI_LINUX_DMA_FENCE_H
#define _LINUXKPI_LINUX_DMA_FENCE_H

#include <linux/types.h>
#include <linux/kref.h>
#include <linux/spinlock.h>
#include <linux/rcupdate.h>
#include <linux/ktime.h>
#include <linux/bitops.h>  /* test_bit — dma_fence_timestamp reads DMA_FENCE_FLAG_TIMESTAMP_BIT */
/* The DRM scheduler reaches rb_node + the scope-guard machinery transitively
 * through dma-fence in the real kernel; mirror that so gpu_scheduler.h typechecks. */
#include <linux/rbtree.h>
#include <linux/cleanup.h>
#include <linux/export.h>

struct dma_fence;
struct dma_fence_cb;
struct dma_fence_ops;

typedef void (*dma_fence_func_t)(struct dma_fence *fence, struct dma_fence_cb *cb);

struct dma_fence_cb {
	struct list_head node;
	dma_fence_func_t func;
};

struct dma_fence {
	spinlock_t *lock;
	const struct dma_fence_ops *ops;
	/* the kernel overlaps these three across the fence lifetime */
	union {
		struct list_head cb_list;
		ktime_t timestamp;
		struct rcu_head rcu;
	};
	u64 context;
	u64 seqno;
	unsigned long flags;
	struct kref refcount;
	int error;
};

/* fence->flags bits (kernel ABI) */
enum dma_fence_flag_bits {
	DMA_FENCE_FLAG_SIGNALED_BIT,
	DMA_FENCE_FLAG_TIMESTAMP_BIT,
	DMA_FENCE_FLAG_ENABLE_SIGNAL_BIT,
	DMA_FENCE_FLAG_USER_BITS, /* must be last */
};

struct dma_fence_ops {
	bool use_64bit_seqno;
	const char *(*get_driver_name)(struct dma_fence *fence);
	const char *(*get_timeline_name)(struct dma_fence *fence);
	bool (*enable_signaling)(struct dma_fence *fence);
	bool (*signaled)(struct dma_fence *fence);
	signed long (*wait)(struct dma_fence *fence, bool intr, signed long timeout);
	void (*release)(struct dma_fence *fence);
	void (*fence_value_str)(struct dma_fence *fence, char *str, int size);
	void (*timeline_value_str)(struct dma_fence *fence, char *str, int size);
	void (*set_deadline)(struct dma_fence *fence, ktime_t deadline);
};

#define DMA_FENCE_TRACE(f, fmt, args...) do { } while (0)
/* sentinel timeout: wait "forever" */
#define MAX_SCHEDULE_TIMEOUT ((long)(~0UL >> 1))

/* ---- lifecycle + signalling: real machinery, backed by raeen_linuxkpi (M4) ---- */
void dma_fence_init(struct dma_fence *fence, const struct dma_fence_ops *ops,
		    spinlock_t *lock, u64 context, u64 seqno);
struct dma_fence *dma_fence_get(struct dma_fence *fence);
struct dma_fence *dma_fence_get_rcu(struct dma_fence *fence);
struct dma_fence *dma_fence_get_rcu_safe(struct dma_fence __rcu **fencep);
void dma_fence_put(struct dma_fence *fence);
void dma_fence_free(struct dma_fence *fence);
void dma_fence_release(struct kref *kref);

int  dma_fence_signal(struct dma_fence *fence);
int  dma_fence_signal_locked(struct dma_fence *fence);
int  dma_fence_signal_timestamp(struct dma_fence *fence, ktime_t timestamp);
bool dma_fence_is_signaled(struct dma_fence *fence);
bool dma_fence_is_signaled_locked(struct dma_fence *fence);
void dma_fence_enable_sw_signaling(struct dma_fence *fence);

int  dma_fence_add_callback(struct dma_fence *fence, struct dma_fence_cb *cb,
			    dma_fence_func_t func);
bool dma_fence_remove_callback(struct dma_fence *fence, struct dma_fence_cb *cb);

signed long dma_fence_wait_timeout(struct dma_fence *fence, bool intr, signed long timeout);
signed long dma_fence_default_wait(struct dma_fence *fence, bool intr, signed long timeout);
struct dma_fence *dma_fence_get_stub(void);
struct dma_fence *dma_fence_allocate_private_stub(ktime_t timestamp);
static inline signed long dma_fence_wait(struct dma_fence *fence, bool intr)
{
	signed long ret = dma_fence_wait_timeout(fence, intr, MAX_SCHEDULE_TIMEOUT);
	return ret < 0 ? ret : 0;
}

/* ---- pure accessors (the only meaning the names carry) ---- */
static inline void dma_fence_set_error(struct dma_fence *fence, int error) { fence->error = error; }
static inline int  dma_fence_get_status_locked(struct dma_fence *fence) { return fence->error; }

/* dma_fence_set_deadline: forward a hint to the fence's own deadline handler,
 * if it has one (upstream <linux/dma-fence.h> inline, verbatim). Drivers whose
 * ops don't implement set_deadline simply never get the hint — matches
 * upstream, not a fake. */
static inline void dma_fence_set_deadline(struct dma_fence *fence, ktime_t deadline)
{
	if (fence->ops->set_deadline)
		fence->ops->set_deadline(fence, deadline);
}

/* dma_fence_timestamp: the time the fence was signaled (upstream
 * <linux/dma-fence.h> inline, verbatim algorithm) — reads the `timestamp`
 * union member if the signal path recorded one, else falls back to "now"
 * (the documented race: out-of-order interrupt signaling can beat the
 * timestamp write). Locks the fence's own spinlock, same as upstream. */
static inline ktime_t dma_fence_timestamp(struct dma_fence *fence)
{
	unsigned long flags;
	ktime_t timestamp;

	if (!dma_fence_is_signaled(fence))
		return ktime_get();

	spin_lock_irqsave(fence->lock, flags);
	if (test_bit(DMA_FENCE_FLAG_TIMESTAMP_BIT, &fence->flags))
		timestamp = fence->timestamp;
	else
		timestamp = ktime_get();
	spin_unlock_irqrestore(fence->lock, flags);

	return timestamp;
}

/* later-than test honouring 32- vs 64-bit seqno wrap (the documented contract). */
static inline bool __dma_fence_is_later(u64 f1, u64 f2, const struct dma_fence_ops *ops)
{
	if (ops && ops->use_64bit_seqno)
		return f1 > f2;
	return (int)(((u32)f1) - ((u32)f2)) > 0;
}
static inline bool dma_fence_is_later(struct dma_fence *f1, struct dma_fence *f2)
{
	if (f1->context != f2->context)
		return false;
	return __dma_fence_is_later(f1->seqno, f2->seqno, f1->ops);
}

#endif /* _LINUXKPI_LINUX_DMA_FENCE_H */
