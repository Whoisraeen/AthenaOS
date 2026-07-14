/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/spinlock.h> shim (MPL-2.0, original work).
 *
 * A REAL spinlock — test-and-set over the GCC atomic builtins, giving genuine
 * mutual exclusion (NOT a no-op fake: amdgpu's ring/fence/IRQ paths rely on it,
 * and the DRM scheduler runs worker threads, so the exclusion must be real —
 * SCOPE.md rule 9). The `_irqsave`/`_irq` variants additionally "disable IRQs"
 * in the kernel; in the userspace-daemon model there are no kernel interrupts to
 * mask (device IRQs arrive as IPC, not an interrupt frame), so those variants are
 * the same real lock and the saved-flags value is unused (honestly 0).
 *
 * License boundary (../../README.md): the spinlock API surface over compiler
 * atomics — no GPL source.
 */
#ifndef _LINUXKPI_LINUX_SPINLOCK_H
#define _LINUXKPI_LINUX_SPINLOCK_H

#include <linux/types.h>
/* real <linux/spinlock.h> transitively reaches lockdep_assert_held() and its
 * siblings (locking + the deadlock-checker are always paired upstream) — some
 * callers (drm/scheduler's sched_main.c) use lockdep_assert_held() without
 * including <linux/lockdep.h> directly, same as upstream. Without this the
 * no-op macro is never defined in that TU, so the raw name falls through to
 * an implicit (unbacked) function declaration — a compile-vs-link-time trap,
 * not a missing feature. */
#include <linux/lockdep.h>

typedef struct { volatile int lock; } spinlock_t;
/* the kernel spells the raw lock both as the typedef and as `struct raw_spinlock`
 * (drm_mode_config embeds the struct form by value), so define the struct. */
struct raw_spinlock { volatile int lock; };
typedef struct raw_spinlock raw_spinlock_t;
/* reader/writer spinlock — same real exclusion (serialises readers too; the
 * kernel's shared-read parallelism is an M5 refinement, like rwsem.h). */
typedef struct { volatile int lock; } rwlock_t;

#define DEFINE_SPINLOCK(name)     spinlock_t name = { 0 }
#define DEFINE_RAW_SPINLOCK(name) raw_spinlock_t name = { 0 }
#define __SPIN_LOCK_UNLOCKED(x)   { 0 }

static inline void spin_lock_init(spinlock_t *l)      { __atomic_store_n(&l->lock, 0, __ATOMIC_RELAXED); }

static inline void spin_lock(spinlock_t *l)
{
	int expected;
	do {
		expected = 0;
	} while (!__atomic_compare_exchange_n(&l->lock, &expected, 1, false,
					      __ATOMIC_ACQUIRE, __ATOMIC_RELAXED));
}
static inline void spin_unlock(spinlock_t *l) { __atomic_store_n(&l->lock, 0, __ATOMIC_RELEASE); }
static inline int  spin_trylock(spinlock_t *l)
{
	int expected = 0;
	return __atomic_compare_exchange_n(&l->lock, &expected, 1, false,
					   __ATOMIC_ACQUIRE, __ATOMIC_RELAXED) ? 1 : 0;
}

/* IRQ-context variants: real lock; "flags" carries no IRQ state in userspace. */
#define spin_lock_irqsave(l, flags)      do { (flags) = 0; spin_lock(l); } while (0)
#define spin_unlock_irqrestore(l, flags) do { (void)(flags); spin_unlock(l); } while (0)
#define spin_lock_irq(l)    spin_lock(l)
#define spin_unlock_irq(l)  spin_unlock(l)
#define spin_lock_bh(l)     spin_lock(l)
#define spin_unlock_bh(l)   spin_unlock(l)

/* raw_spinlock: same real test-and-set, operating on `struct raw_spinlock`. */
static inline void raw_spin_lock_init(raw_spinlock_t *l) { __atomic_store_n(&l->lock, 0, __ATOMIC_RELAXED); }
static inline void raw_spin_lock(raw_spinlock_t *l)
{
	int expected;
	do { expected = 0; }
	while (!__atomic_compare_exchange_n(&l->lock, &expected, 1, false,
					    __ATOMIC_ACQUIRE, __ATOMIC_RELAXED));
}
static inline void raw_spin_unlock(raw_spinlock_t *l) { __atomic_store_n(&l->lock, 0, __ATOMIC_RELEASE); }
#define raw_spin_lock_irqsave(l, f)      do { (f) = 0; raw_spin_lock(l); } while (0)
#define raw_spin_unlock_irqrestore(l, f) do { (void)(f); raw_spin_unlock(l); } while (0)
#define raw_spin_lock_irq(l)    raw_spin_lock(l)
#define raw_spin_unlock_irq(l)  raw_spin_unlock(l)

/* rwlock: real exclusion (serialises readers too — M5 may make reads parallel). */
#define rwlock_init(l)     __atomic_store_n(&(l)->lock, 0, __ATOMIC_RELAXED)
static inline void __rwlock_acquire(rwlock_t *l)
{
	int expected;
	do { expected = 0; }
	while (!__atomic_compare_exchange_n(&l->lock, &expected, 1, false,
					    __ATOMIC_ACQUIRE, __ATOMIC_RELAXED));
}
static inline void __rwlock_release(rwlock_t *l) { __atomic_store_n(&l->lock, 0, __ATOMIC_RELEASE); }
#define read_lock(l)    __rwlock_acquire(l)
#define read_unlock(l)  __rwlock_release(l)
#define write_lock(l)   __rwlock_acquire(l)
#define write_unlock(l) __rwlock_release(l)
#define read_lock_irqsave(l, f)       do { (f) = 0; __rwlock_acquire(l); } while (0)
#define read_unlock_irqrestore(l, f)  do { (void)(f); __rwlock_release(l); } while (0)
#define write_lock_irqsave(l, f)      do { (f) = 0; __rwlock_acquire(l); } while (0)
#define write_unlock_irqrestore(l, f) do { (void)(f); __rwlock_release(l); } while (0)

#define assert_spin_locked(l) do { } while (0)
#define spin_is_locked(l)     (__atomic_load_n(&(l)->lock, __ATOMIC_RELAXED) != 0)

#endif /* _LINUXKPI_LINUX_SPINLOCK_H */
