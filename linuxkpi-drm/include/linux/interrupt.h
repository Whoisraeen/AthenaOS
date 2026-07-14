/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/interrupt.h> shim (MPL-2.0, original work).
 *
 * IRQ registration + deferred (tasklet) work. amdgpu registers its IH ring
 * handler via request_irq and defers bottom-half work to tasklets. In the AthenaOS
 * userspace-daemon model a device IRQ is delivered as IPC (ath_linuxkpi's
 * IRQ-doorbell facade, P2), so request_irq wires the handler to that delivery and
 * tasklets run on the daemon's deferred-work pump — both backed at M4, never
 * faked (a no-op request_irq would mean the GPU's interrupts never reach the
 * driver; SCOPE.md rule 9). License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_INTERRUPT_H
#define _LINUXKPI_LINUX_INTERRUPT_H

#include <linux/types.h>
#include <linux/preempt.h>   /* in_interrupt() */
#include <linux/irqreturn.h> /* irqreturn_t (shared with direct includers) */

typedef irqreturn_t (*irq_handler_t)(int irq, void *dev_id);

#define IRQF_SHARED       0x00000080
#define IRQF_ONESHOT      0x00002000
#define IRQF_NOBALANCING  0x00000800
#define IRQF_TRIGGER_NONE 0x00000000

/* IRQ registration — backed by ath_linuxkpi's doorbell facade (M4) */
int  request_irq(unsigned int irq, irq_handler_t handler, unsigned long flags, const char *name, void *dev);
int  request_threaded_irq(unsigned int irq, irq_handler_t handler, irq_handler_t thread_fn,
			  unsigned long flags, const char *name, void *dev);
void free_irq(unsigned int irq, void *dev_id);
void enable_irq(unsigned int irq);
void disable_irq(unsigned int irq);
void disable_irq_nosync(unsigned int irq);
void synchronize_irq(unsigned int irq);

/* tasklets (deferred bottom-half) — backed by the M4 deferred-work pump */
struct tasklet_struct {
	struct tasklet_struct *next;
	unsigned long          state;
	atomic_t               count;
	void (*callback)(struct tasklet_struct *t);
	void (*func)(unsigned long data);
	unsigned long          data;
};
void tasklet_init(struct tasklet_struct *t, void (*func)(unsigned long), unsigned long data);
void tasklet_setup(struct tasklet_struct *t, void (*callback)(struct tasklet_struct *));
void tasklet_schedule(struct tasklet_struct *t);
void tasklet_hi_schedule(struct tasklet_struct *t);
void tasklet_kill(struct tasklet_struct *t);
void tasklet_enable(struct tasklet_struct *t);
void tasklet_disable(struct tasklet_struct *t);

#endif /* _LINUXKPI_LINUX_INTERRUPT_H */
