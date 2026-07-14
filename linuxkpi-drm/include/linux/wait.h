/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/wait.h> shim (MPL-2.0, original work).
 *
 * Wait queues — block-until-condition + wake. amdgpu/DRM use them for fence and
 * IB-completion waits and the scheduler's idle wait. The `wait_event*` family
 * loop-checks the condition and blocks on an M4-backed waiter between checks
 * (REAL block/wake — not a busy-spin and not a no-op that returns immediately,
 * SCOPE.md rule 9); `wake_up*` and the low-level block are backed by
 * raeen_linuxkpi at M4. License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_WAIT_H
#define _LINUXKPI_LINUX_WAIT_H

#include <linux/types.h>
#include <linux/spinlock.h>

struct wait_queue_head {
	spinlock_t        lock;
	struct list_head  head;
};
typedef struct wait_queue_head wait_queue_head_t;

struct wait_queue_entry {
	unsigned int      flags;
	void             *private;
	struct list_head  entry;
};
typedef struct wait_queue_entry wait_queue_entry_t;

#define __WAIT_QUEUE_HEAD_INITIALIZER(name) { .head = { &(name).head, &(name).head } }
#define DECLARE_WAIT_QUEUE_HEAD(name) wait_queue_head_t name = __WAIT_QUEUE_HEAD_INITIALIZER(name)

static inline void init_waitqueue_head(wait_queue_head_t *wq)
{ spin_lock_init(&wq->lock); wq->head.next = wq->head.prev = &wq->head; }

/* low-level block/wake — backed by raeen_linuxkpi (M4). __wait_block parks the
 * caller on `wq` until a wake_up targets it; the _timeout form returns the
 * jiffies left (0 on timeout). */
void wake_up(wait_queue_head_t *wq);
void wake_up_all(wait_queue_head_t *wq);
void wake_up_interruptible(wait_queue_head_t *wq);
void wake_up_interruptible_all(wait_queue_head_t *wq);
void __wait_block(wait_queue_head_t *wq);
long __wait_block_timeout(wait_queue_head_t *wq, long timeout);
void add_wait_queue(wait_queue_head_t *wq, wait_queue_entry_t *entry);
void remove_wait_queue(wait_queue_head_t *wq, wait_queue_entry_t *entry);

#define wait_event(wq, condition) \
	do { while (!(condition)) __wait_block(&(wq)); } while (0)
#define wait_event_interruptible(wq, condition) \
	({ while (!(condition)) __wait_block(&(wq)); 0; })
#define wait_event_timeout(wq, condition, timeout) \
	({ long __wet = (timeout); \
	   while (!(condition) && __wet > 0) __wet = __wait_block_timeout(&(wq), __wet); \
	   (condition) ? (__wet ? __wet : 1) : 0; })
#define wait_event_interruptible_timeout(wq, condition, timeout) \
	wait_event_timeout(wq, condition, timeout)
#define wait_event_killable(wq, condition) wait_event_interruptible(wq, condition)

#endif /* _LINUXKPI_LINUX_WAIT_H */
