/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/smp.h> shim (MPL-2.0, original work).
 *
 * Per-CPU / cross-CPU helpers. amdgpu uses smp_processor_id() for a few per-cpu
 * fast paths and on_each_cpu() for cache/TLB-style fan-out. The CPU id + the
 * cross-CPU call are backed by raeen_linuxkpi at M4 (a constant-0 processor id
 * would alias per-cpu state across the daemon's threads, and a no-op on_each_cpu
 * would skip required work — both real, not faked; SCOPE.md rule 9). get_cpu/
 * put_cpu pair with the (no-op) userspace preempt gate. License boundary: surface.
 */
#ifndef _LINUXKPI_LINUX_SMP_H
#define _LINUXKPI_LINUX_SMP_H

#include <linux/types.h>
#include <linux/preempt.h>

/* real CPU id + topology — backed by raeen_linuxkpi (M4) */
unsigned int raw_smp_processor_id(void);
unsigned int num_online_cpus(void);
unsigned int num_possible_cpus(void);
unsigned int num_present_cpus(void);
extern unsigned int nr_cpu_ids;

#define smp_processor_id() raw_smp_processor_id()
#define get_cpu()  ({ preempt_disable(); raw_smp_processor_id(); })
#define put_cpu()  preempt_enable()

/* cross-CPU function call — backed by raeen_linuxkpi (M4) */
void on_each_cpu(void (*func)(void *info), void *info, int wait);
int  smp_call_function_single(int cpu, void (*func)(void *info), void *info, int wait);
void smp_call_function(void (*func)(void *info), void *info, int wait);

#endif /* _LINUXKPI_LINUX_SMP_H */
