/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/completion.h> shim (MPL-2.0, original work).
 *
 * One-shot "wait until done" primitive. amdgpu/DRM use it for reset handshakes,
 * worker-thread teardown, and firmware-load rendezvous.
 *
 * `init_completion`/`reinit_completion` are pure state resets (inlined). The
 * BLOCKING side (complete/wait_for_completion) must actually block-and-wake — a
 * no-op wait that returned immediately would be a silent-success fake breaking
 * every handshake (SCOPE.md rule 9) — so it is declaration-only, backed by
 * ath_linuxkpi's wait facade at M4. amdgpu touches the struct only through this
 * API, so the internals are opaque. License boundary (../../README.md): surface.
 */
#ifndef _LINUXKPI_LINUX_COMPLETION_H
#define _LINUXKPI_LINUX_COMPLETION_H

#include <linux/types.h>
#include <linux/spinlock.h>
#include <linux/wait.h>   /* completion is built on a wait queue (kernel parity) */
/* real <linux/completion.h> transitively reaches task_struct/current (via the
 * sched/signal header chain) — callers like drm/scheduler's sched_entity.c use
 * `current`/PF_EXITING/SIGKILL without including <linux/sched.h> directly, same
 * as upstream. */
#include <linux/sched.h>

struct completion {
	unsigned int done;
	spinlock_t   wait_lock; /* guards `done` + the M4 wait queue */
};

#define DECLARE_COMPLETION(name)         struct completion name = { 0 }
#define DECLARE_COMPLETION_ONSTACK(name) struct completion name = { 0 }

static inline void init_completion(struct completion *x)   { x->done = 0; spin_lock_init(&x->wait_lock); }
static inline void reinit_completion(struct completion *x) { x->done = 0; }

/* blocking side — real block/wake, backed by ath_linuxkpi (M4) */
void complete(struct completion *x);
void complete_all(struct completion *x);
void wait_for_completion(struct completion *x);
int  wait_for_completion_interruptible(struct completion *x);
unsigned long wait_for_completion_timeout(struct completion *x, unsigned long timeout);
long wait_for_completion_interruptible_timeout(struct completion *x, unsigned long timeout);
bool try_wait_for_completion(struct completion *x);
bool completion_done(struct completion *x);

#endif /* _LINUXKPI_LINUX_COMPLETION_H */
