/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/xarray.h> shim (MPL-2.0, original work).
 *
 * The kernel's resizable sparse array (radix-tree successor). The DRM scheduler
 * and amdgpu use it for id->object maps (ctx handles, fence contexts, BO ids).
 *
 * `xa_init*` is a pure reset (inlined). The store/load/alloc/erase + iteration
 * machinery is a real data structure with allocation + locking, so it is
 * declaration-only, backed by ath_linuxkpi at M4 — NOT faked to "always empty"
 * (that would silently lose every stored object, SCOPE.md rule 9). The mark/value
 * tag helpers are pure bit-twiddling and inlined. License boundary: API surface.
 */
#ifndef _LINUXKPI_LINUX_XARRAY_H
#define _LINUXKPI_LINUX_XARRAY_H

#include <linux/types.h>
#include <linux/spinlock.h>
/* real <linux/xarray.h> transitively reaches GFP_KERNEL/gfp_t (via slab.h) —
 * xa_alloc/xa_store take a gfp_t and callers pass GFP_KERNEL without including
 * <linux/slab.h> directly, same as upstream. */
#include <linux/slab.h>

#ifndef ULONG_MAX
#define ULONG_MAX (~0UL)
#endif

struct xarray {
	spinlock_t    xa_lock;
	unsigned int  xa_flags;
	void         *xa_head;
};

struct xa_limit { u32 max; u32 min; };
#define XA_LIMIT(_min, _max) (struct xa_limit) { .min = (_min), .max = (_max) }
/* the common "any 32-bit ID" allocation range (amdgpu/drm-scheduler idiom). */
#define xa_limit_32b XA_LIMIT(0, UINT_MAX)

#define XA_FLAGS_LOCK_IRQ   ((gfp_t)1U << 0)
#define XA_FLAGS_LOCK_BH    ((gfp_t)1U << 1)
#define XA_FLAGS_TRACK_FREE ((gfp_t)1U << 2)
#define XA_FLAGS_ALLOC      XA_FLAGS_TRACK_FREE
#define XA_FLAGS_ALLOC1     (XA_FLAGS_TRACK_FREE | ((gfp_t)1U << 3))

#define XA_PRESENT ((unsigned int)8)

#define DEFINE_XARRAY_FLAGS(name, flags) \
	struct xarray name = { .xa_lock = { 0 }, .xa_flags = (flags), .xa_head = (void *)0 }
#define DEFINE_XARRAY(name)        DEFINE_XARRAY_FLAGS(name, 0)
#define DEFINE_XARRAY_ALLOC(name)  DEFINE_XARRAY_FLAGS(name, XA_FLAGS_ALLOC)
#define DEFINE_XARRAY_ALLOC1(name) DEFINE_XARRAY_FLAGS(name, XA_FLAGS_ALLOC1)

static inline void xa_init_flags(struct xarray *xa, gfp_t flags)
{
	spin_lock_init(&xa->xa_lock);
	xa->xa_flags = flags;
	xa->xa_head = (void *)0;
}
static inline void xa_init(struct xarray *xa) { xa_init_flags(xa, 0); }

#define xa_lock(xa)    spin_lock(&(xa)->xa_lock)
#define xa_unlock(xa)  spin_unlock(&(xa)->xa_lock)
#define xa_lock_irqsave(xa, f)      spin_lock_irqsave(&(xa)->xa_lock, f)
#define xa_unlock_irqrestore(xa, f) spin_unlock_irqrestore(&(xa)->xa_lock, f)

/* error/value tagging — pure pointer bit-twiddling (kernel ABI). */
static inline void *xa_mk_value(unsigned long v)  { return (void *)((v << 1) | 1); }
static inline unsigned long xa_to_value(const void *e) { return (unsigned long)e >> 1; }
static inline bool xa_is_value(const void *e)     { return (unsigned long)e & 1; }
static inline void *xa_mk_internal(unsigned long v) { return (void *)((v << 2) | 2); }
static inline bool xa_is_err(const void *e)       { return (unsigned long)e > (unsigned long)-4096; }
static inline int  xa_err(void *e)                { return xa_is_err(e) ? (int)(long)e : 0; }

/* data structure ops — backed by ath_linuxkpi (M4) */
void *xa_load(struct xarray *xa, unsigned long index);
void *xa_store(struct xarray *xa, unsigned long index, void *entry, gfp_t gfp);
void *xa_erase(struct xarray *xa, unsigned long index);
void *__xa_erase(struct xarray *xa, unsigned long index);
void *xa_cmpxchg(struct xarray *xa, unsigned long index, void *old, void *entry, gfp_t gfp);
int   xa_alloc(struct xarray *xa, u32 *id, void *entry, struct xa_limit limit, gfp_t gfp);
int   xa_alloc_cyclic(struct xarray *xa, u32 *id, void *entry, struct xa_limit limit,
		      u32 *next, gfp_t gfp);
#define xa_alloc_cyclic_irq(xa, id, entry, limit, next, gfp) \
	xa_alloc_cyclic((xa), (id), (entry), (limit), (next), (gfp))
void  xa_destroy(struct xarray *xa);
bool  xa_empty(const struct xarray *xa);
void *xa_find(struct xarray *xa, unsigned long *index, unsigned long max, unsigned int filter);
void *xa_find_after(struct xarray *xa, unsigned long *index, unsigned long max, unsigned int filter);

#define xa_for_each(xa, index, entry) \
	for ((index) = 0, (entry) = xa_find((xa), &(index), ULONG_MAX, XA_PRESENT); \
	     (entry); \
	     (entry) = xa_find_after((xa), &(index), ULONG_MAX, XA_PRESENT))
#define xa_for_each_start(xa, index, entry, start) \
	for ((index) = (start), (entry) = xa_find((xa), &(index), ULONG_MAX, XA_PRESENT); \
	     (entry); \
	     (entry) = xa_find_after((xa), &(index), ULONG_MAX, XA_PRESENT))

#endif /* _LINUXKPI_LINUX_XARRAY_H */
