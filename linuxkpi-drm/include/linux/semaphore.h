/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/semaphore.h> shim (MPL-2.0, original work).
 *
 * Counting semaphore. The DRM task_barrier (used by amdgpu reset to rendezvous all
 * the per-IP threads) is built on it. REAL counting semantics over the atomic
 * builtins — down() genuinely waits for a unit, up() releases one (not a no-op
 * fake; SCOPE.md rule 9). It busy-acquires for self-containment; M5 may swap to a
 * blocking wait. No daemon-init signal interrupts, so _interruptible always
 * acquires. License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_SEMAPHORE_H
#define _LINUXKPI_LINUX_SEMAPHORE_H

#include <linux/types.h>
#include <linux/atomic.h>

struct semaphore { atomic_t count; };

#define DEFINE_SEMAPHORE(name) struct semaphore name = { .count = ATOMIC_INIT(1) }

static inline void sema_init(struct semaphore *sem, int val) { atomic_set(&sem->count, val); }

static inline int down_trylock(struct semaphore *sem)
{
	int c = atomic_read(&sem->count);
	while (c > 0) {
		if (__atomic_compare_exchange_n(&sem->count.counter, &c, c - 1, false,
						__ATOMIC_ACQUIRE, __ATOMIC_RELAXED))
			return 0;       /* acquired */
	}
	return 1;                       /* would block */
}
static inline void down(struct semaphore *sem) { while (down_trylock(sem)) { /* spin */ } }
static inline int  down_interruptible(struct semaphore *sem) { down(sem); return 0; }
static inline int  down_killable(struct semaphore *sem)      { down(sem); return 0; }
static inline void up(struct semaphore *sem) { atomic_inc(&sem->count); }

#endif /* _LINUXKPI_LINUX_SEMAPHORE_H */
