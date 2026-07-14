/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/rcupdate.h> shim (MPL-2.0, original work).
 *
 * Read-Copy-Update surface. amdgpu/DRM use RCU for lockless fence + xarray reads
 * and deferred frees (dma_fence embeds an `rcu_head`).
 *
 * Userspace-daemon model: the read-side critical section exists to fence against
 * preemption/grace-periods; with no kernel preemption to gate, rcu_read_lock/
 * unlock are honest NO-OPS. The DEFERRED-FREE side (call_rcu/synchronize_rcu)
 * has real ordering obligations — freeing too early is a use-after-free — so it
 * is declaration-only, backed by ath_linuxkpi's grace-period machinery at M4.
 * NOT faked to free immediately (SCOPE.md rule 9). License boundary: API surface.
 */
#ifndef _LINUXKPI_LINUX_RCUPDATE_H
#define _LINUXKPI_LINUX_RCUPDATE_H

#include <linux/types.h>
#include <linux/atomic.h>   /* for the access barriers */

/* The kernel calls this `callback_head`; `rcu_head` is the historical alias. */
struct rcu_head {
	struct rcu_head *next;
	void (*func)(struct rcu_head *head);
};

/* read-side: no kernel preemption in the daemon -> no-op critical section. */
#define rcu_read_lock()        do { } while (0)
#define rcu_read_unlock()      do { } while (0)
#define rcu_read_lock_sched()  do { } while (0)
#define rcu_read_unlock_sched() do { } while (0)
#define rcu_read_lock_held()   (1)

/* publish/consume: a plain access plus the matching barrier (the real RCU
 * ordering contract on a strongly-ordered ISA). */
#define rcu_dereference(p)            ({ __typeof__(p) ___p = (p); rmb(); ___p; })
#define rcu_dereference_raw(p)        (p)
#define rcu_dereference_protected(p, c) (p)
/* rcu_dereference_check: like rcu_dereference_protected, plus a lockdep
 * condition `c` the real macro asserts before the access. lockdep is
 * compiled out (lockdep_assert_held is a no-op here), so `c` is unevaluated —
 * matches the CONFIG_LOCKDEP=n posture already documented above the read-side
 * no-ops in this file, not a narrower fake. */
#define rcu_dereference_check(p, c)   (p)
#define rcu_access_pointer(p)         (p)
#define rcu_assign_pointer(p, v)      do { wmb(); (p) = (v); } while (0)
#define RCU_INIT_POINTER(p, v)        do { (p) = (v); } while (0)

/* deferred free / grace period — real obligations, backed by ath_linuxkpi (M4) */
void call_rcu(struct rcu_head *head, void (*func)(struct rcu_head *head));
void synchronize_rcu(void);
#define kfree_rcu(ptr, rcu_member) call_rcu(&(ptr)->rcu_member, (void (*)(struct rcu_head *))0)

#define __rcu
#define __force

#endif /* _LINUXKPI_LINUX_RCUPDATE_H */
