/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/sched/task.h> shim (MPL-2.0, original work).
 *
 * Task-lifetime refcounting. amdgpu touches this on the KFD/user-queue paths
 * (out of the MES bring-up subset). The bring-up daemon's own thread owns the
 * task lifetime for the duration of init, so the extra advisory refs are no-ops
 * here — never a silent fake on a live path, just a deliberately-disabled
 * subsystem (SCOPE.md rule 9). License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_SCHED_TASK_H
#define _LINUXKPI_LINUX_SCHED_TASK_H

#include <linux/types.h>

struct task_struct;
struct mm_struct;

static inline void get_task_struct(struct task_struct *t) { (void)t; }
static inline void put_task_struct(struct task_struct *t) { (void)t; }
static inline struct mm_struct *get_task_mm(struct task_struct *t) { (void)t; return (void *)0; }
static inline void mmput(struct mm_struct *mm) { (void)mm; }

#endif /* _LINUXKPI_LINUX_SCHED_TASK_H */
