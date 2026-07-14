/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/kthread.h> shim (MPL-2.0, original work).
 *
 * Kernel threads. amdgpu/KFD run worker kthreads (the GPU scheduler's run-queue
 * thread, reset worker). Backed by raeen_linuxkpi's thread model at M4 — a fake
 * kthread_run that spawned nothing would mean the scheduler never drains
 * (SCOPE.md rule 9). License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_KTHREAD_H
#define _LINUXKPI_LINUX_KTHREAD_H

#include <linux/types.h>

struct task_struct;

/* thread lifecycle — backed by raeen_linuxkpi (M4) */
struct task_struct *kthread_create_on_node(int (*threadfn)(void *data), void *data, int node,
					   const char *namefmt, ...);
struct task_struct *kthread_create(int (*threadfn)(void *data), void *data, const char *namefmt, ...);
int  wake_up_process(struct task_struct *p);
int  kthread_stop(struct task_struct *k);
bool kthread_should_stop(void);
bool kthread_should_park(void);
int  kthread_park(struct task_struct *k);
void kthread_unpark(struct task_struct *k);
void kthread_bind(struct task_struct *k, unsigned int cpu);

#define kthread_run(threadfn, data, namefmt, ...) \
	({ \
		struct task_struct *__k = kthread_create(threadfn, data, namefmt, ##__VA_ARGS__); \
		if (__k) wake_up_process(__k); \
		__k; \
	})

#endif /* _LINUXKPI_LINUX_KTHREAD_H */
