/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/rwsem.h> shim (MPL-2.0, original work).
 *
 * Reader/writer semaphore. amdgpu protects its VM + reset state with these. This
 * is a REAL lock (genuine mutual exclusion over <linux/mutex.h>); for simplicity
 * it serialises readers too (a read lock takes the same exclusive lock). That is
 * correct — just less concurrent than the kernel's true shared-read rwsem; M5 may
 * upgrade to reader-parallel. NOT a no-op fake (SCOPE.md rule 9). License boundary
 * (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_RWSEM_H
#define _LINUXKPI_LINUX_RWSEM_H

#include <linux/types.h>
#include <linux/mutex.h>

struct rw_semaphore { struct mutex lock; };

#define DECLARE_RWSEM(name) struct rw_semaphore name = { }
#define DEFINE_RWSEM(name)  struct rw_semaphore name = { }

static inline void init_rwsem(struct rw_semaphore *sem) { mutex_init(&sem->lock); }
static inline void down_read(struct rw_semaphore *sem)  { mutex_lock(&sem->lock); }
static inline void up_read(struct rw_semaphore *sem)    { mutex_unlock(&sem->lock); }
static inline void down_write(struct rw_semaphore *sem) { mutex_lock(&sem->lock); }
static inline void up_write(struct rw_semaphore *sem)   { mutex_unlock(&sem->lock); }
static inline int  down_read_trylock(struct rw_semaphore *sem)  { return mutex_trylock(&sem->lock); }
static inline int  down_write_trylock(struct rw_semaphore *sem) { return mutex_trylock(&sem->lock); }
static inline void downgrade_write(struct rw_semaphore *sem)    { (void)sem; /* already held exclusively */ }
static inline int  rwsem_is_locked(struct rw_semaphore *sem)    { return mutex_is_locked(&sem->lock); }

#endif /* _LINUXKPI_LINUX_RWSEM_H */
