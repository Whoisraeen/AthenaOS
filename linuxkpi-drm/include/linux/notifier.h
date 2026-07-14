/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/notifier.h> shim (MPL-2.0, original work).
 *
 * Notifier chains — ordered callback broadcast. amdgpu hooks reset/hotplug/
 * panic notifiers. The chain register/unregister/call is backed by ath_linuxkpi
 * at M4 (real ordered list + locking; a no-op register would silently drop every
 * notification — SCOPE.md rule 9). License boundary (../../README.md): surface.
 */
#ifndef _LINUXKPI_LINUX_NOTIFIER_H
#define _LINUXKPI_LINUX_NOTIFIER_H

#include <linux/types.h>
#include <linux/spinlock.h>

struct notifier_block;
typedef int (*notifier_fn_t)(struct notifier_block *nb, unsigned long action, void *data);

struct notifier_block {
	notifier_fn_t          notifier_call;
	struct notifier_block *next;
	int                    priority;
};

struct atomic_notifier_head   { spinlock_t lock; struct notifier_block *head; };
struct blocking_notifier_head { struct notifier_block *head; };
struct raw_notifier_head      { struct notifier_block *head; };

#define ATOMIC_NOTIFIER_HEAD(name)   struct atomic_notifier_head name = { .head = (void *)0 }
#define BLOCKING_NOTIFIER_HEAD(name) struct blocking_notifier_head name = { .head = (void *)0 }
#define RAW_NOTIFIER_HEAD(name)      struct raw_notifier_head name = { .head = (void *)0 }

#define NOTIFY_DONE     0x0000
#define NOTIFY_OK       0x0001
#define NOTIFY_STOP_MASK 0x8000
#define NOTIFY_BAD      (NOTIFY_STOP_MASK | 0x0002)
#define NOTIFY_STOP     (NOTIFY_OK | NOTIFY_STOP_MASK)

/* chain ops — backed by ath_linuxkpi (M4) */
int atomic_notifier_chain_register(struct atomic_notifier_head *nh, struct notifier_block *nb);
int atomic_notifier_chain_unregister(struct atomic_notifier_head *nh, struct notifier_block *nb);
int atomic_notifier_call_chain(struct atomic_notifier_head *nh, unsigned long val, void *v);
int blocking_notifier_chain_register(struct blocking_notifier_head *nh, struct notifier_block *nb);
int blocking_notifier_chain_unregister(struct blocking_notifier_head *nh, struct notifier_block *nb);
int blocking_notifier_call_chain(struct blocking_notifier_head *nh, unsigned long val, void *v);

#endif /* _LINUXKPI_LINUX_NOTIFIER_H */
