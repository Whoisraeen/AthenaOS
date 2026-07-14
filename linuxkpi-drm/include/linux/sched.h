/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/sched.h> shim (MPL-2.0, original work).
 *
 * Task + scheduling surface. CRITICAL: this header MUST exist as a shim so that
 * any `#include <linux/sched.h>` in the real DRM/amdgpu headers resolves HERE and
 * not to the host's /usr/include/linux/sched.h (which would mix host kernel ABI
 * into the build). amdgpu uses `current`, task comm/pid, TASK_* states, and the
 * yield/schedule helpers. The scheduling actions (schedule/cond_resched) are
 * backed by raeen_linuxkpi at M4; `current` resolves to the daemon's task. NOT a
 * no-op fake (rule 9). License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_SCHED_H
#define _LINUXKPI_LINUX_SCHED_H

#include <linux/types.h>
/* capable()/CAP_* are reached through <linux/sched.h> in much driver code (the
 * check keys off the current task's creds); pull it so CAP_SYS_NICE et al resolve. */
#include <linux/capability.h>
/* real <linux/sched.h> transitively pulls signal.h (task exit_code vs SIGKILL). */
#include <linux/signal.h>
/* atom.c (ATOMBIOS interpreter) includes <linux/sched.h> then atom.h, which
 * embeds `struct mutex` by value — pull the full mutex definition transitively
 * (the real sched.h drags it in via task_struct). */
#include <linux/mutex.h>

#define TASK_COMM_LEN 16

#define TASK_RUNNING            0x0000
#define TASK_INTERRUPTIBLE      0x0001
#define TASK_UNINTERRUPTIBLE    0x0002
#define TASK_NORMAL             (TASK_INTERRUPTIBLE | TASK_UNINTERRUPTIBLE)
#define TASK_DEAD               0x0080
#define MAX_SCHEDULE_TIMEOUT    ((long)(~0UL >> 1))

/* task_struct->flags bit (PF_*) the DRM scheduler checks on entity teardown to
 * tell a normal fini from a dying-process fini (drm_sched_entity_flush). */
#define PF_EXITING               0x00000004

struct task_struct {
	int   prio;
	pid_t pid;
	pid_t tgid;
	char  comm[TASK_COMM_LEN];
	void *mm;
	unsigned int flags;
	int   exit_code;                   /* set on process exit (e.g. SIGKILL) */
	struct task_struct *group_leader;  /* thread-group leader (process owner) */
};

struct pid {
	pid_t nr;
};

enum pid_type {
	PIDTYPE_PID,
	PIDTYPE_TGID,
};

struct pid *task_tgid(struct task_struct *task);
struct pid *get_pid(struct pid *pid);
void put_pid(struct pid *pid);
struct task_struct *pid_task(struct pid *pid, enum pid_type type);
pid_t pid_nr(struct pid *pid);
pid_t task_pid_nr(struct task_struct *task);

/* the running task — backed by raeen_linuxkpi (M4) */
struct task_struct *get_current(void);
#define current (get_current())

/* copy a task's comm[] into a caller buffer (amdgpu records the VM owner). */
char *get_task_comm(char *buf, struct task_struct *tsk);

/* scheduling actions — backed by raeen_linuxkpi (M4) */
void schedule(void);
long schedule_timeout(long timeout);
long io_schedule_timeout(long timeout);
int  cond_resched(void);
void yield(void);
void set_current_state(unsigned int state);
void __set_current_state(unsigned int state);
int  signal_pending(struct task_struct *p);
int  fatal_signal_pending(struct task_struct *p);

#define need_resched()  (0)
#define might_sleep()   do { } while (0)
#define might_sleep_if(cond) do { } while (0)

#endif /* _LINUXKPI_LINUX_SCHED_H */
