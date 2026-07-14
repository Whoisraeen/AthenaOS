/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/ratelimit.h> shim (MPL-2.0, original work).
 *
 * Message/event rate limiter. amdgpu embeds `struct ratelimit_state` BY VALUE for
 * its RAS error-throttle counters (the `_rs` fields), so the type must be defined
 * for layout. The rate-limit decision is backed by ath_linuxkpi at M4; here it
 * is the struct + init. License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_RATELIMIT_H
#define _LINUXKPI_LINUX_RATELIMIT_H

#include <linux/types.h>
#include <linux/spinlock.h>

struct ratelimit_state {
	raw_spinlock_t lock;
	int            interval;
	int            burst;
	int            printed;
	int            missed;
	unsigned long  begin;
	unsigned long  flags;
};

#define RATELIMIT_MSG_ON_RELEASE (1 << 0)

#define DEFINE_RATELIMIT_STATE(name, interval_init, burst_init) \
	struct ratelimit_state name = { .interval = (interval_init), .burst = (burst_init) }

static inline void ratelimit_state_init(struct ratelimit_state *rs, int interval, int burst)
{ raw_spin_lock_init(&rs->lock); rs->interval = interval; rs->burst = burst; rs->printed = 0; rs->missed = 0; }

/* rate-limit decision — backed by ath_linuxkpi (M4) */
int ___ratelimit(struct ratelimit_state *rs, const char *func);
#define __ratelimit(state) ___ratelimit(state, __func__)

#endif /* _LINUXKPI_LINUX_RATELIMIT_H */
