/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/sched/mm.h> shim (MPL-2.0, original work).
 *
 * mm_struct refcounting + address-space locking, used by amdgpu's userptr/VM
 * path. Not on the MES bring-up path (userptr is out of subset); reached via
 * amdgpu_vm.h for type/decl layout. Backed by raeen_linuxkpi at M4 when userptr
 * is in scope. License boundary (../../README.md): API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_SCHED_MM_H
#define _LINUXKPI_LINUX_SCHED_MM_H

#include <linux/types.h>

struct mm_struct;
struct task_struct;

/* mm refcounting — backed by raeen_linuxkpi (M4) */
struct mm_struct *get_task_mm(struct task_struct *task);
void mmput(struct mm_struct *mm);
void mmget(struct mm_struct *mm);
bool mmget_not_zero(struct mm_struct *mm);
void mmgrab(struct mm_struct *mm);
void mmdrop(struct mm_struct *mm);

/* mmap_lock helpers — backed by raeen_linuxkpi (M4) */
void mmap_read_lock(struct mm_struct *mm);
int  mmap_read_lock_killable(struct mm_struct *mm);
void mmap_read_unlock(struct mm_struct *mm);
void mmap_write_lock(struct mm_struct *mm);
void mmap_write_unlock(struct mm_struct *mm);

#endif /* _LINUXKPI_LINUX_SCHED_MM_H */
