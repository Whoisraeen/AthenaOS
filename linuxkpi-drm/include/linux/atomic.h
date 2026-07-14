/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/atomic.h> shim (MPL-2.0, original work).
 *
 * The kernel atomic API, mapped onto the C11/GCC `__atomic_*` builtins exactly
 * as FreeBSD's LinuxKPI does. These are REAL atomic operations (the genuine
 * semantics the names promise), not fakes — `atomic_inc()` really increments
 * atomically. amdgpu uses these pervasively (refcounts, ring head/tail, fence
 * seqnos, IRQ-disable nesting).
 *
 * Ordering: the kernel's non-returning ops are relaxed; the `_return`/`fetch_`/
 * test ops carry a full barrier. We map reads/sets to RELAXED and every RMW to
 * SEQ_CST — strictly stronger than the kernel contract, hence always correct
 * (a bring-up shim favours correctness over the last bit of ordering economy).
 *
 * License boundary (../../README.md): this is the atomic API *surface* expressed
 * over compiler primitives — no GPL source.
 */
#ifndef _LINUXKPI_LINUX_ATOMIC_H
#define _LINUXKPI_LINUX_ATOMIC_H

#include <linux/types.h>

/* The kernel wraps the counter in a struct so a bare int can't be passed where
 * an atomic_t is expected — preserve that (amdgpu relies on the type check). */
typedef struct { int counter; }     atomic_t;
typedef struct { s64 counter; }      atomic64_t;
typedef atomic64_t                   atomic_long_t;

#define ATOMIC_INIT(i)   { (i) }
#define ATOMIC64_INIT(i) { (i) }

/* ---- memory barriers ---- */
#define mb()   __atomic_thread_fence(__ATOMIC_SEQ_CST)
#define rmb()  __atomic_thread_fence(__ATOMIC_ACQUIRE)
#define wmb()  __atomic_thread_fence(__ATOMIC_RELEASE)
#define smp_mb()  mb()
#define smp_rmb() rmb()
#define smp_wmb() wmb()
#define smp_mb__before_atomic() smp_mb()
#define smp_mb__after_atomic()  smp_mb()
#define dma_rmb()  rmb()
#define dma_wmb()  wmb()
#define smp_read_barrier_depends() do { } while (0)

/* ---- 32-bit atomic_t ---- */
static inline int  atomic_read(const atomic_t *v) { return __atomic_load_n(&v->counter, __ATOMIC_RELAXED); }
static inline void atomic_set(atomic_t *v, int i)  { __atomic_store_n(&v->counter, i, __ATOMIC_RELAXED); }
static inline void atomic_add(int i, atomic_t *v)  { (void)__atomic_add_fetch(&v->counter, i, __ATOMIC_SEQ_CST); }
static inline void atomic_sub(int i, atomic_t *v)  { (void)__atomic_sub_fetch(&v->counter, i, __ATOMIC_SEQ_CST); }
static inline void atomic_inc(atomic_t *v)         { (void)__atomic_add_fetch(&v->counter, 1, __ATOMIC_SEQ_CST); }
static inline void atomic_dec(atomic_t *v)         { (void)__atomic_sub_fetch(&v->counter, 1, __ATOMIC_SEQ_CST); }
static inline int  atomic_add_return(int i, atomic_t *v) { return __atomic_add_fetch(&v->counter, i, __ATOMIC_SEQ_CST); }
static inline int  atomic_sub_return(int i, atomic_t *v) { return __atomic_sub_fetch(&v->counter, i, __ATOMIC_SEQ_CST); }
static inline int  atomic_inc_return(atomic_t *v)  { return __atomic_add_fetch(&v->counter, 1, __ATOMIC_SEQ_CST); }
static inline int  atomic_dec_return(atomic_t *v)  { return __atomic_sub_fetch(&v->counter, 1, __ATOMIC_SEQ_CST); }
static inline int  atomic_fetch_add(int i, atomic_t *v) { return __atomic_fetch_add(&v->counter, i, __ATOMIC_SEQ_CST); }
static inline int  atomic_fetch_sub(int i, atomic_t *v) { return __atomic_fetch_sub(&v->counter, i, __ATOMIC_SEQ_CST); }
static inline int  atomic_fetch_or(int i, atomic_t *v)  { return __atomic_fetch_or(&v->counter, i, __ATOMIC_SEQ_CST); }
static inline int  atomic_fetch_and(int i, atomic_t *v) { return __atomic_fetch_and(&v->counter, i, __ATOMIC_SEQ_CST); }
static inline void atomic_and(int i, atomic_t *v)  { (void)__atomic_and_fetch(&v->counter, i, __ATOMIC_SEQ_CST); }
static inline void atomic_or(int i, atomic_t *v)   { (void)__atomic_or_fetch(&v->counter, i, __ATOMIC_SEQ_CST); }
static inline int  atomic_xchg(atomic_t *v, int n) { return __atomic_exchange_n(&v->counter, n, __ATOMIC_SEQ_CST); }
static inline int  atomic_cmpxchg(atomic_t *v, int old, int n)
{
	__atomic_compare_exchange_n(&v->counter, &old, n, false,
				    __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST);
	return old; /* the value found (== old on success) — kernel semantics */
}
static inline bool atomic_dec_and_test(atomic_t *v) { return atomic_dec_return(v) == 0; }
static inline bool atomic_inc_and_test(atomic_t *v) { return atomic_inc_return(v) == 0; }
static inline bool atomic_sub_and_test(int i, atomic_t *v) { return atomic_sub_return(i, v) == 0; }
static inline bool atomic_add_negative(int i, atomic_t *v) { return atomic_add_return(i, v) < 0; }
static inline int  atomic_add_unless(atomic_t *v, int a, int u)
{
	int c = atomic_read(v);
	while (c != u && !__atomic_compare_exchange_n(&v->counter, &c, c + a, false,
						      __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST))
		;
	return c;
}
static inline bool atomic_inc_not_zero(atomic_t *v) { return atomic_add_unless(v, 1, 0) != 0; }

/* ---- 64-bit atomic64_t ---- */
static inline s64  atomic64_read(const atomic64_t *v) { return __atomic_load_n(&v->counter, __ATOMIC_RELAXED); }
static inline void atomic64_set(atomic64_t *v, s64 i)  { __atomic_store_n(&v->counter, i, __ATOMIC_RELAXED); }
static inline void atomic64_add(s64 i, atomic64_t *v)  { (void)__atomic_add_fetch(&v->counter, i, __ATOMIC_SEQ_CST); }
static inline void atomic64_sub(s64 i, atomic64_t *v)  { (void)__atomic_sub_fetch(&v->counter, i, __ATOMIC_SEQ_CST); }
static inline void atomic64_inc(atomic64_t *v)         { (void)__atomic_add_fetch(&v->counter, 1, __ATOMIC_SEQ_CST); }
static inline void atomic64_dec(atomic64_t *v)         { (void)__atomic_sub_fetch(&v->counter, 1, __ATOMIC_SEQ_CST); }
static inline s64  atomic64_add_return(s64 i, atomic64_t *v) { return __atomic_add_fetch(&v->counter, i, __ATOMIC_SEQ_CST); }
static inline s64  atomic64_sub_return(s64 i, atomic64_t *v) { return __atomic_sub_fetch(&v->counter, i, __ATOMIC_SEQ_CST); }
static inline s64  atomic64_inc_return(atomic64_t *v)  { return __atomic_add_fetch(&v->counter, 1, __ATOMIC_SEQ_CST); }
static inline s64  atomic64_dec_return(atomic64_t *v)  { return __atomic_sub_fetch(&v->counter, 1, __ATOMIC_SEQ_CST); }
static inline s64  atomic64_xchg(atomic64_t *v, s64 n) { return __atomic_exchange_n(&v->counter, n, __ATOMIC_SEQ_CST); }
static inline s64  atomic64_cmpxchg(atomic64_t *v, s64 old, s64 n)
{
	__atomic_compare_exchange_n(&v->counter, &old, n, false,
				    __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST);
	return old;
}
/* atomic_long_t IS atomic64_t on this (64-bit) target — same op, long-named. */
static inline long atomic_long_xchg(atomic_long_t *v, long n) { return atomic64_xchg(v, n); }

/* ---- type-generic cmpxchg/xchg on plain lvalues ---- */
#define cmpxchg(ptr, o, n) __sync_val_compare_and_swap((ptr), (o), (n))
#define xchg(ptr, n)       __atomic_exchange_n((ptr), (n), __ATOMIC_SEQ_CST)
#define cmpxchg64(ptr, o, n) cmpxchg((ptr), (o), (n))

/* ---- acquire/release single-variable ordering (type-generic, GCC builtins —
 * same idiom as xchg/cmpxchg above). The DRM scheduler uses these for its
 * lock-free pause/paused_entities-style flags. */
#define smp_load_acquire(p)     __atomic_load_n((p), __ATOMIC_ACQUIRE)
#define smp_store_release(p, v) __atomic_store_n((p), (v), __ATOMIC_RELEASE)

#endif /* _LINUXKPI_LINUX_ATOMIC_H */
