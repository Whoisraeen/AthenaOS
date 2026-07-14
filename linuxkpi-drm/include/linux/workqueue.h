/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/workqueue.h> shim (MPL-2.0, original work).
 *
 * Deferred work. The DRM scheduler and amdgpu run reset/hotplug/fence-signal work
 * off workqueues. Backed by ath_linuxkpi's PUMP-DRIVEN workqueue facade at M4
 * (the daemon drains queued work each loop). The struct layout is chosen to match
 * that facade: `func` sits at OFFSET 24 (atomic_long_t data @0..8, list_head
 * entry @8..24, func @24) — the facade reads the callback there.
 *
 * INIT_WORK is a pure field init (inlined). queue/cancel/flush are real scheduling
 * + completion ops (declaration-only, backed at M4) — a no-op enqueue that never
 * ran the work would silently drop resets/signals (SCOPE.md rule 9). License
 * boundary (../../README.md): API surface, layout chosen for the facade ABI.
 */
#ifndef _LINUXKPI_LINUX_WORKQUEUE_H
#define _LINUXKPI_LINUX_WORKQUEUE_H

#include <linux/types.h>
#include <linux/atomic.h>
#include <linux/timer.h>

struct work_struct;
typedef void (*work_func_t)(struct work_struct *work);

struct work_struct {
	atomic_long_t    data;   /* offset 0  */
	struct list_head entry;  /* offset 8  */
	work_func_t      func;    /* offset 24 — the facade reads the callback here */
};

struct delayed_work {
	struct work_struct      work;
	struct timer_list       timer;
	struct workqueue_struct *wq;
	int                     cpu;
};

struct workqueue_struct; /* opaque — owned by the M4 facade */
struct work_struct;

#define WQ_UNBOUND   (1 << 1)
#define WQ_HIGHPRI   (1 << 4)
#define WQ_MEM_RECLAIM (1 << 3)

/* pure field-init (the kernel's __INIT_WORK, self-contained so we don't pull list.h) */
#define INIT_WORK(_work, _func) do {                          \
		(_work)->func = (_func);                      \
		(_work)->entry.next = &(_work)->entry;        \
		(_work)->entry.prev = &(_work)->entry;        \
		atomic_long_set(&(_work)->data, 0);           \
	} while (0)
#define INIT_WORK_ONSTACK(_work, _func) INIT_WORK(_work, _func)
#define INIT_DELAYED_WORK(_dwork, _func) do {                 \
		INIT_WORK(&(_dwork)->work, (_func));          \
		timer_setup(&(_dwork)->timer, (void (*)(struct timer_list *))0, 0); \
	} while (0)
#define atomic_long_set(v, i) atomic64_set((v), (i))

#define work_pending(work) (0)
#define to_delayed_work(_w) ((struct delayed_work *)(_w))

/* scheduling/cancel/flush — real, backed by ath_linuxkpi (M4) */
struct workqueue_struct *alloc_workqueue(const char *fmt, unsigned int flags, int max_active, ...);
struct workqueue_struct *alloc_ordered_workqueue(const char *fmt, unsigned int flags, ...);
void destroy_workqueue(struct workqueue_struct *wq);

bool queue_work(struct workqueue_struct *wq, struct work_struct *work);
bool queue_delayed_work(struct workqueue_struct *wq, struct delayed_work *dwork, unsigned long delay);
bool mod_delayed_work(struct workqueue_struct *wq, struct delayed_work *dwork, unsigned long delay);
bool schedule_work(struct work_struct *work);
bool schedule_delayed_work(struct delayed_work *dwork, unsigned long delay);
bool cancel_work_sync(struct work_struct *work);
bool cancel_delayed_work(struct delayed_work *dwork);
bool cancel_delayed_work_sync(struct delayed_work *dwork);
bool flush_work(struct work_struct *work);
bool flush_delayed_work(struct delayed_work *dwork);
void flush_workqueue(struct workqueue_struct *wq);
void drain_workqueue(struct workqueue_struct *wq);

extern struct workqueue_struct *system_wq;
extern struct workqueue_struct *system_highpri_wq;
extern struct workqueue_struct *system_unbound_wq;
/* 7.0.x renamed the default system workqueue system_wq -> system_percpu_wq
 * (drm/scheduler's drm_sched_init reads this one; same single-pump facade). */
extern struct workqueue_struct *system_percpu_wq;

#endif /* _LINUXKPI_LINUX_WORKQUEUE_H */
