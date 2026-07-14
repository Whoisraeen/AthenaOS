/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/irq_work.h> shim (MPL-2.0, original work).
 *
 * Deferred work run in hard-IRQ context. The composite-fence types
 * (dma_fence_array/chain) embed a `struct irq_work` BY VALUE for their async
 * signal callback, so the type must be fully defined for layout. On the bring-up
 * daemon there is no hard-IRQ context — the work runs inline via the cooperative
 * pump (workqueue.rs) — so init/queue are backed by ath_linuxkpi at M4.
 * License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_IRQ_WORK_H
#define _LINUXKPI_LINUX_IRQ_WORK_H

#include <linux/types.h>
#include <linux/llist.h>

struct irq_work {
	struct llist_node llnode;
	unsigned long     flags;
	void (*func)(struct irq_work *);
};

#define IRQ_WORK_INIT(_func) { .func = (_func) }

static inline void init_irq_work(struct irq_work *work, void (*func)(struct irq_work *))
{
	work->llnode.next = (void *)0;
	work->flags = 0;
	work->func = func;
}

/* queue/sync — backed by ath_linuxkpi (M4); the daemon pump drains them. */
bool irq_work_queue(struct irq_work *work);
void irq_work_sync(struct irq_work *work);

#endif /* _LINUXKPI_LINUX_IRQ_WORK_H */
