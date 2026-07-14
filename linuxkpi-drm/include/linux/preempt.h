/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/preempt.h> shim (MPL-2.0, original work).
 *
 * Kernel preemption / atomic-context control. amdgpu (and the DRM spsc queue)
 * wrap lock-free sections in preempt_disable()/enable() and branch on
 * in_interrupt()/in_atomic().
 *
 * In the RaeenOS userspace-daemon model there is no kernel preemption to gate, so
 * the disable/enable calls are honest NO-OPS (not fakes — there is genuinely no
 * kernel scheduler to hold off here, the same posture as module.h's macros).
 * The context predicates report "normal task context" because the daemon always
 * runs as one. License boundary (../../README.md): API surface only.
 */
#ifndef _LINUXKPI_LINUX_PREEMPT_H
#define _LINUXKPI_LINUX_PREEMPT_H

#include <linux/types.h>

#define preempt_disable()            do { } while (0)
#define preempt_enable()             do { } while (0)
#define preempt_enable_no_resched()  do { } while (0)
#define preempt_disable_notrace()    do { } while (0)
#define preempt_enable_notrace()     do { } while (0)
#define migrate_disable()            do { } while (0)
#define migrate_enable()             do { } while (0)

#define preempt_count()   (0)
#define preemptible()     (1)   /* a userspace task is always preemptible */

/* Context predicates: the daemon is always in normal task context, never in a
 * hardirq/softirq/atomic section (IRQ delivery arrives as IPC, not a kernel
 * interrupt frame), so these are constant. */
#define in_interrupt()  (0)
#define in_irq()        (0)
#define in_softirq()    (0)
#define in_atomic()     (0)
#define in_task()       (1)
#define irqs_disabled() (0)

#endif /* _LINUXKPI_LINUX_PREEMPT_H */
