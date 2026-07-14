/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/dma-resv.h> shim (MPL-2.0, original work).
 *
 * Per-buffer reservation object: a ww-mutex plus the set of fences (kernel/write/
 * read/bookkeep usages) that must complete before the buffer is reused. Every GEM
 * BO embeds one; amdgpu syncs submissions against it.
 *
 * The LOCK is real (ww_mutex — genuine exclusion + the modeset/exec ww ordering).
 * The FENCE-SET management (add/iterate/test/wait) is backed by raeen_linuxkpi at
 * M4: a fake that dropped fences or reported "always signalled" would let the GPU
 * stomp a buffer still in flight (SCOPE.md rule 9). License boundary: API surface.
 */
#ifndef _LINUXKPI_LINUX_DMA_RESV_H
#define _LINUXKPI_LINUX_DMA_RESV_H

#include <linux/types.h>
#include <linux/ww_mutex.h>
#include <linux/dma-fence.h>
#include <linux/rcupdate.h>

enum dma_resv_usage {
	DMA_RESV_USAGE_KERNEL,
	DMA_RESV_USAGE_WRITE,
	DMA_RESV_USAGE_READ,
	DMA_RESV_USAGE_BOOKKEEP,
};

/* dma_resv_usage_rw: the usage a submission implies for a buffer it's about to
 * write vs. only read (upstream <linux/dma-resv.h> inline, verbatim). */
static inline enum dma_resv_usage dma_resv_usage_rw(bool write)
{
	return write ? DMA_RESV_USAGE_WRITE : DMA_RESV_USAGE_READ;
}

struct dma_resv_list;

struct dma_resv {
	struct ww_mutex      lock;
	struct dma_resv_list *fences; /* managed by the M4 facade */
};

extern struct ww_class reservation_ww_class;

/* lock: real ww_mutex exclusion (inline). */
static inline int  dma_resv_lock(struct dma_resv *obj, struct ww_acquire_ctx *ctx) { return ww_mutex_lock(&obj->lock, ctx); }
static inline int  dma_resv_lock_interruptible(struct dma_resv *obj, struct ww_acquire_ctx *ctx) { return ww_mutex_lock_interruptible(&obj->lock, ctx); }
static inline void dma_resv_lock_slow(struct dma_resv *obj, struct ww_acquire_ctx *ctx) { ww_mutex_lock_slow(&obj->lock, ctx); }
static inline bool dma_resv_trylock(struct dma_resv *obj) { return ww_mutex_trylock(&obj->lock, (void *)0) != 0; }
static inline void dma_resv_unlock(struct dma_resv *obj) { ww_mutex_unlock(&obj->lock); }
static inline bool dma_resv_is_locked(struct dma_resv *obj) { return ww_mutex_is_locked(&obj->lock); }
static inline bool dma_resv_held(struct dma_resv *obj) { return ww_mutex_is_locked(&obj->lock); }
#define dma_resv_assert_held(obj) do { } while (0)

/* fence-set management — backed by raeen_linuxkpi (M4). */
void dma_resv_init(struct dma_resv *obj);
void dma_resv_fini(struct dma_resv *obj);
int  dma_resv_reserve_fences(struct dma_resv *obj, unsigned int num_fences);
void dma_resv_add_fence(struct dma_resv *obj, struct dma_fence *fence, enum dma_resv_usage usage);
void dma_resv_replace_fences(struct dma_resv *obj, u64 context, struct dma_fence *replacement, enum dma_resv_usage usage);
int  dma_resv_get_singleton(struct dma_resv *obj, enum dma_resv_usage usage, struct dma_fence **fence);
long dma_resv_wait_timeout(struct dma_resv *obj, enum dma_resv_usage usage, bool intr, unsigned long timeout);
bool dma_resv_test_signaled(struct dma_resv *obj, enum dma_resv_usage usage);
int  dma_resv_copy_fences(struct dma_resv *dst, struct dma_resv *src);

/* iteration — backed by raeen_linuxkpi (M4). */
struct dma_resv_iter {
	struct dma_resv     *obj;
	enum dma_resv_usage  usage;
	struct dma_fence    *fence;
	enum dma_resv_usage  fence_usage;
	unsigned int         index;
	struct dma_resv_list *fences;
	bool                 is_restarted;
};
void dma_resv_iter_begin(struct dma_resv_iter *cursor, struct dma_resv *obj, enum dma_resv_usage usage);
void dma_resv_iter_end(struct dma_resv_iter *cursor);
struct dma_fence *dma_resv_iter_first(struct dma_resv_iter *cursor);
struct dma_fence *dma_resv_iter_next(struct dma_resv_iter *cursor);

#define dma_resv_for_each_fence(cursor, obj, usage, fence) \
	for (dma_resv_iter_begin((cursor), (obj), (usage)), \
	     (fence) = dma_resv_iter_first(cursor); \
	     (fence); (fence) = dma_resv_iter_next(cursor))

/* RCU-only walk (no resv lock held); the cursor must already be _begin'd.
 * amdgpu_sync uses this to snapshot a BO's fences without taking the resv lock. */
struct dma_fence *dma_resv_iter_first_unlocked(struct dma_resv_iter *cursor);
struct dma_fence *dma_resv_iter_next_unlocked(struct dma_resv_iter *cursor);
#define dma_resv_for_each_fence_unlocked(cursor, fence) \
	for ((fence) = dma_resv_iter_first_unlocked(cursor); \
	     (fence); (fence) = dma_resv_iter_next_unlocked(cursor))

#endif /* _LINUXKPI_LINUX_DMA_RESV_H */
