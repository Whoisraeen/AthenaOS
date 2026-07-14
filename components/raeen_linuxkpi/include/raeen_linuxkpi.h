/* AthenaOS LinuxKPI — C ABI for userspace Linux driver ports (Phase 1).
 * SPDX-License-Identifier: MPL-2.0
 *
 * Link this static library into a driver daemon; the kernel host syscalls
 * back timing and logging. Heap allocations use an in-library bump allocator
 * until Phase 2 routes large DMA buffers through capabilities.
 */
#ifndef RAEEN_LINUXKPI_H
#define RAEEN_LINUXKPI_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define RAEEN_LINUXKPI_ABI_VERSION 1u

/* Linux-compatible GFP flags (subset). */
#define GFP_KERNEL 0x0u
#define GFP_ATOMIC 0x1u
#define GFP_ZERO   0x100u

void *kmalloc(size_t size, unsigned int flags);
void *kzalloc(size_t size, unsigned int flags);
void kfree(const void *ptr);

unsigned long long get_jiffies_64(void);
void msleep(unsigned int msecs);

/* Fixed-string printk for bring-up; printf-style deferred to Phase 2. */
int raeen_printk(const char *msg);

/* Spinlock stubs — map to atomics in Phase 1 (always succeed). */
typedef struct {
    volatile uint32_t lock;
} spinlock_t;

#define spin_lock_init(_lock) ((_lock)->lock = 0)
void spin_lock(spinlock_t *lock);
void spin_unlock(spinlock_t *lock);

#ifdef __cplusplus
}
#endif

#endif /* RAEEN_LINUXKPI_H */
