/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/kref.h> shim (MPL-2.0, original work).
 *
 * The kernel's reference-count helper, built on a REAL atomic counter (see
 * atomic.h) — genuine refcount semantics, not a fake. amdgpu and the DRM core
 * use kref for object lifetimes (dma_fence, drm_device, gem objects).
 *
 * License boundary (../../README.md): API surface over atomic.h — no GPL source.
 */
#ifndef _LINUXKPI_LINUX_KREF_H
#define _LINUXKPI_LINUX_KREF_H

#include <linux/types.h>
#include <linux/atomic.h>

struct kref { atomic_t refcount; };

static inline void kref_init(struct kref *kref) { atomic_set(&kref->refcount, 1); }
static inline unsigned int kref_read(const struct kref *kref) { return (unsigned int)atomic_read(&kref->refcount); }
static inline void kref_get(struct kref *kref) { atomic_inc(&kref->refcount); }

/* Drop a reference; when it reaches 0 run `release` and report 1 (freed). */
static inline int kref_put(struct kref *kref, void (*release)(struct kref *kref))
{
	if (atomic_dec_and_test(&kref->refcount)) {
		release(kref);
		return 1;
	}
	return 0;
}

static inline int kref_get_unless_zero(struct kref *kref)
{
	return atomic_inc_not_zero(&kref->refcount);
}

#endif /* _LINUXKPI_LINUX_KREF_H */
