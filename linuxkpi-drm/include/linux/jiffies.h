/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/jiffies.h> shim (MPL-2.0, original work).
 *
 * The kernel tick counter + time-comparison helpers. amdgpu's SMU/reset code uses
 * `jiffies` + time_after() for bounded waits and msecs_to_jiffies() for timeouts.
 * `jiffies` is backed by raeen_linuxkpi's timing facade at M4 (a real, advancing
 * tick — a frozen counter would hang every timeout loop, SCOPE.md rule 9). The
 * conversions + comparisons are pure. License boundary (../../README.md): surface.
 */
#ifndef _LINUXKPI_LINUX_JIFFIES_H
#define _LINUXKPI_LINUX_JIFFIES_H

#include <linux/types.h>

#ifndef HZ
#define HZ 1000        /* tick rate: 1 jiffy = 1 ms */
#endif

/* the live tick counter — backed by raeen_linuxkpi (M4) */
extern volatile unsigned long jiffies;
u64 get_jiffies_64(void);

#define msecs_to_jiffies(m)  ((unsigned long)(m) * HZ / 1000)
#define usecs_to_jiffies(u)  ((unsigned long)(u) * HZ / 1000000)
#define jiffies_to_msecs(j)  ((unsigned int)((j) * 1000 / HZ))
#define jiffies_to_usecs(j)  ((unsigned int)((j) * 1000000 / HZ))
#define secs_to_jiffies(s)   ((unsigned long)(s) * HZ)

/* signed wrap-safe comparisons (the documented time_after contract). */
#define time_after(a, b)     ((long)((b) - (a)) < 0)
#define time_before(a, b)    time_after(b, a)
#define time_after_eq(a, b)  ((long)((a) - (b)) >= 0)
#define time_before_eq(a, b) time_after_eq(b, a)
#define time_in_range(a, b, c) (time_after_eq(a, b) && time_before_eq(a, c))

#endif /* _LINUXKPI_LINUX_JIFFIES_H */
