/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/mutex.h> shim (MPL-2.0, original work).
 *
 * Sleeping mutual-exclusion lock. amdgpu serialises most of its init + reset
 * paths under mutexes (the GMC, PSP, SMU, ring-mux locks). This is a REAL mutex
 * (atomic acquire/release giving genuine exclusion — not a no-op fake, SCOPE.md
 * rule 9). It busy-acquires here for self-containment; M5 may swap the body to
 * ath_linuxkpi's blocking mutex so long holds (firmware load) don't spin a core.
 * No signals interrupt the daemon's init, so the _interruptible/_killable
 * variants honestly always acquire (return 0). License boundary: API surface.
 */
#ifndef _LINUXKPI_LINUX_MUTEX_H
#define _LINUXKPI_LINUX_MUTEX_H

#include <linux/types.h>
#include <linux/cleanup.h>   /* DEFINE_GUARD -> guard(mutex) / scoped_guard(mutex) */

struct mutex { volatile int locked; };

#define DEFINE_MUTEX(name) struct mutex name = { 0 }

static inline void mutex_init(struct mutex *m) { __atomic_store_n(&m->locked, 0, __ATOMIC_RELAXED); }
static inline void mutex_destroy(struct mutex *m) { (void)m; }
static inline int  mutex_is_locked(struct mutex *m) { return __atomic_load_n(&m->locked, __ATOMIC_RELAXED) != 0; }

static inline void mutex_lock(struct mutex *m)
{
	int expected;
	do {
		expected = 0;
	} while (!__atomic_compare_exchange_n(&m->locked, &expected, 1, false,
					      __ATOMIC_ACQUIRE, __ATOMIC_RELAXED));
}
static inline void mutex_unlock(struct mutex *m) { __atomic_store_n(&m->locked, 0, __ATOMIC_RELEASE); }
static inline int  mutex_trylock(struct mutex *m)
{
	int expected = 0;
	return __atomic_compare_exchange_n(&m->locked, &expected, 1, false,
					   __ATOMIC_ACQUIRE, __ATOMIC_RELAXED) ? 1 : 0;
}
/* no daemon-init signal can interrupt these -> they always acquire (0 = success) */
static inline int  mutex_lock_interruptible(struct mutex *m) { mutex_lock(m); return 0; }
static inline int  mutex_lock_killable(struct mutex *m)      { mutex_lock(m); return 0; }
/* lockdep nesting subclass is irrelevant without lockdep — plain lock. */
#define mutex_lock_nested(lock, subclass) mutex_lock(lock)

/* scope-based mutex guard: `guard(mutex)(&lock)` / `scoped_guard(mutex, &lock)`.
 * Instantiates class_mutex_t + its constructor/destructor (cleanup.h). */
DEFINE_GUARD(mutex, struct mutex *, mutex_lock(_T), mutex_unlock(_T))

#endif /* _LINUXKPI_LINUX_MUTEX_H */
