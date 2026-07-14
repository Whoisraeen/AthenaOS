/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/hrtimer.h> shim (MPL-2.0, original work).
 *
 * High-resolution timer. amdgpu arms these for VRR/vblank pacing and precise
 * delays. The callback struct uses the upstream member names amdgpu sets
 * (`function`, `_softexpires`); arming/cancel is backed by ath_linuxkpi's timer
 * facade at M4 (a no-op that never fired would stall vblank waits — SCOPE.md
 * rule 9). License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_HRTIMER_H
#define _LINUXKPI_LINUX_HRTIMER_H

#include <linux/types.h>
#include <linux/ktime.h>

enum hrtimer_restart { HRTIMER_NORESTART, HRTIMER_RESTART };
enum hrtimer_mode {
	HRTIMER_MODE_ABS = 0x00,
	HRTIMER_MODE_REL = 0x01,
	HRTIMER_MODE_PINNED = 0x02,
	HRTIMER_MODE_ABS_PINNED = 0x02,
	HRTIMER_MODE_REL_PINNED = 0x03,
};
enum hrtimer_base_type { CLOCK_MONOTONIC_BASE, CLOCK_REALTIME_BASE };

struct hrtimer {
	ktime_t _softexpires;
	enum hrtimer_restart (*function)(struct hrtimer *timer);
	int   clock_base;
	void *priv; /* M4 facade handle */
};

void  hrtimer_init(struct hrtimer *timer, int clock_id, enum hrtimer_mode mode);
void  hrtimer_start(struct hrtimer *timer, ktime_t tim, enum hrtimer_mode mode);
void  hrtimer_start_range_ns(struct hrtimer *timer, ktime_t tim, u64 range_ns, enum hrtimer_mode mode);
int   hrtimer_cancel(struct hrtimer *timer);
int   hrtimer_try_to_cancel(struct hrtimer *timer);
bool  hrtimer_active(const struct hrtimer *timer);
u64   hrtimer_forward(struct hrtimer *timer, ktime_t now, ktime_t interval);
u64   hrtimer_forward_now(struct hrtimer *timer, ktime_t interval);

#endif /* _LINUXKPI_LINUX_HRTIMER_H */
