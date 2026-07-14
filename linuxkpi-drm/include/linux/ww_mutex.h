/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/ww_mutex.h> shim (MPL-2.0, original work).
 *
 * Wound/wait mutex — the deadlock-avoiding lock the DRM modeset + GEM reservation
 * paths use to take many object locks in arbitrary order. Reached here only for
 * type layout (drm_modeset_lock); the MES bring-up subset takes no ww locks and
 * runs single-threaded, so it never contends one.
 *
 * The acquire/release give REAL mutual exclusion (over <linux/mutex.h>); the
 * wound/wait ORDERING (the -EDEADLK back-off) is a no-op because there is no
 * multi-lock contention on the MES path — so `ww_mutex_lock` honestly always
 * acquires (returns 0). If display/GEM-reservation code is ever compiled, the
 * full wound/wait algorithm must be restored (M5). Not a fake of a contended
 * path (SCOPE.md rule 9): it is correct for the uncontended use it sees.
 * License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_WW_MUTEX_H
#define _LINUXKPI_LINUX_WW_MUTEX_H

#include <linux/types.h>
#include <linux/mutex.h>

struct ww_class {
	const char *acquire_name;
	const char *mutex_name;
};
struct ww_acquire_ctx {
	struct ww_class *ww_class;
	unsigned long    stamp;
	unsigned         acquired;
};
struct ww_mutex {
	struct mutex     base;
	struct ww_class *ww_class;
	struct ww_acquire_ctx *ctx;
};

#define DEFINE_WW_CLASS(classname) \
	struct ww_class classname = { .acquire_name = #classname "_acquire", .mutex_name = #classname "_mutex" }
#define DEFINE_WW_MUTEX(mutexname, ww_class) \
	struct ww_mutex mutexname = { .ww_class = (ww_class) }

static inline void ww_mutex_init(struct ww_mutex *lock, struct ww_class *ww_class)
{ mutex_init(&lock->base); lock->ww_class = ww_class; lock->ctx = (void *)0; }

static inline void ww_acquire_init(struct ww_acquire_ctx *ctx, struct ww_class *ww_class)
{ ctx->ww_class = ww_class; ctx->stamp = 0; ctx->acquired = 0; }
static inline void ww_acquire_done(struct ww_acquire_ctx *ctx) { (void)ctx; }
static inline void ww_acquire_fini(struct ww_acquire_ctx *ctx) { (void)ctx; }

/* uncontended on the MES path -> always acquires (0); no -EDEADLK back-off. */
static inline int  ww_mutex_lock(struct ww_mutex *lock, struct ww_acquire_ctx *ctx)
{ mutex_lock(&lock->base); lock->ctx = ctx; if (ctx) ctx->acquired++; return 0; }
static inline int  ww_mutex_lock_interruptible(struct ww_mutex *lock, struct ww_acquire_ctx *ctx)
{ return ww_mutex_lock(lock, ctx); }
static inline void ww_mutex_lock_slow(struct ww_mutex *lock, struct ww_acquire_ctx *ctx)
{ (void)ww_mutex_lock(lock, ctx); }
static inline int  ww_mutex_trylock(struct ww_mutex *lock, struct ww_acquire_ctx *ctx)
{ if (mutex_trylock(&lock->base)) { lock->ctx = ctx; return 1; } return 0; }
static inline void ww_mutex_unlock(struct ww_mutex *lock)
{ lock->ctx = (void *)0; mutex_unlock(&lock->base); }
static inline bool ww_mutex_is_locked(struct ww_mutex *lock) { return mutex_is_locked(&lock->base); }

#endif /* _LINUXKPI_LINUX_WW_MUTEX_H */
