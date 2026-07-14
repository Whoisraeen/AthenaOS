/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/delay.h> shim (MPL-2.0, original work).
 *
 * Hardware-timing delays. amdgpu's init/reset paths are full of these — wait for
 * a clock to lock, a fence to post, a microengine to come alive. These MUST be
 * REAL: a no-op delay reads hardware state before it has settled, which is exactly
 * the class of bring-up failure this whole effort exists to fix (SCOPE.md rule 9).
 * So every one is declaration-only, backed by raeen_linuxkpi's timing facade (P1)
 * at M4 — busy-spin for the sub-ms udelay/mdelay, a real sleep for msleep/
 * usleep_range. License boundary (../../README.md): API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_DELAY_H
#define _LINUXKPI_LINUX_DELAY_H

#include <linux/types.h>

/* busy-wait (sub-millisecond, no reschedule) — backed by the M4 timing facade. */
void udelay(unsigned long usecs);
void ndelay(unsigned long nsecs);
void mdelay(unsigned long msecs);

/* sleeping waits (may yield) — backed by the M4 timing facade. */
void msleep(unsigned int msecs);
unsigned long msleep_interruptible(unsigned int msecs);
void usleep_range(unsigned long min, unsigned long max);
void usleep_range_state(unsigned long min, unsigned long max, unsigned int state);
void fsleep(unsigned long usecs);

#endif /* _LINUXKPI_LINUX_DELAY_H */
