/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/timer.h> shim (MPL-2.0, original work).
 *
 * Kernel one-shot timers. amdgpu/DRM arm these for reset watchdogs and the
 * scheduler's job-timeout. Backed by raeen_linuxkpi's timer facade at M4 (it is
 * PUMP-DRIVEN — the daemon calls the run-timers hook each loop), so the arming
 * API is declaration-only here; a no-op timer that never fires would silently
 * disable every watchdog (SCOPE.md rule 9). The struct uses the upstream member
 * names amdgpu reads (`function`, `expires`). License boundary: API surface.
 */
#ifndef _LINUXKPI_LINUX_TIMER_H
#define _LINUXKPI_LINUX_TIMER_H

#include <linux/types.h>

struct timer_list {
	struct hlist_node entry;
	unsigned long     expires;
	void (*function)(struct timer_list *t);
	u32               flags;
};

#define TIMER_IRQSAFE 0x00200000u
#define from_timer(var, callback_timer, timer_fieldname) \
	((void *)((char *)(callback_timer) - offsetof(__typeof__(*(var)), timer_fieldname)))
/* newer kernels renamed from_timer() -> timer_container_of() */
#define timer_container_of(var, callback_timer, timer_fieldname) \
	((__typeof__(var))((char *)(callback_timer) - offsetof(__typeof__(*(var)), timer_fieldname)))

/* setup is a pure field init (the callback fires via the M4 facade). */
static inline void timer_setup(struct timer_list *t,
			       void (*func)(struct timer_list *), unsigned int flags)
{
	t->function = func;
	t->expires = 0;
	t->flags = flags;
	t->entry.next = (struct hlist_node *)0;
}

/* arming/cancel — real scheduling, backed by raeen_linuxkpi (M4) */
int  mod_timer(struct timer_list *timer, unsigned long expires);
void add_timer(struct timer_list *timer);
int  del_timer(struct timer_list *timer);
int  del_timer_sync(struct timer_list *timer);
int  timer_delete_sync(struct timer_list *timer);
int  timer_pending(const struct timer_list *timer);

#endif /* _LINUXKPI_LINUX_TIMER_H */
