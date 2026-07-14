/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/idr.h> shim (MPL-2.0, original work).
 *
 * Integer-ID allocator (IDR) and the lighter ID allocator (IDA), layered on the
 * xarray. DRM uses IDR for object handle->pointer maps; amdgpu uses IDA for ctx
 * and pasid ids. `idr_init*` is a pure reset (inlined); the alloc/find/remove
 * data ops are backed by raeen_linuxkpi at M4 (a fake that handed out colliding
 * or never-freed ids would corrupt the handle space — SCOPE.md rule 9). License
 * boundary (../../README.md): API surface only.
 */
#ifndef _LINUXKPI_LINUX_IDR_H
#define _LINUXKPI_LINUX_IDR_H

#include <linux/types.h>
#include <linux/xarray.h>

struct idr {
	struct xarray idr_rt;
	unsigned int  idr_base;
	unsigned int  idr_next;
};

#define IDR_INIT(name)      { .idr_rt = { 0 }, .idr_base = 0, .idr_next = 0 }
#define DEFINE_IDR(name)    struct idr name = IDR_INIT(name)

static inline void idr_init_base(struct idr *idr, int base) { xa_init_flags(&idr->idr_rt, XA_FLAGS_ALLOC); idr->idr_base = base; idr->idr_next = 0; }
static inline void idr_init(struct idr *idr) { idr_init_base(idr, 0); }
static inline bool idr_is_empty(const struct idr *idr) { return xa_empty(&idr->idr_rt); }

/* data ops — backed by raeen_linuxkpi (M4) */
int   idr_alloc(struct idr *idr, void *ptr, int start, int end, gfp_t gfp);
int   idr_alloc_cyclic(struct idr *idr, void *ptr, int start, int end, gfp_t gfp);
void *idr_remove(struct idr *idr, unsigned long id);
void *idr_find(const struct idr *idr, unsigned long id);
void *idr_replace(struct idr *idr, void *ptr, unsigned long id);
void *idr_get_next(struct idr *idr, int *nextid);
void  idr_destroy(struct idr *idr);
int   idr_for_each(const struct idr *idr, int (*fn)(int id, void *p, void *data), void *data);

#define idr_for_each_entry(idr, entry, id) \
	for ((id) = 0; ((entry) = idr_get_next((idr), &(id))) != (void *)0; (id) += 1)

/* ---- IDA (id bitmap allocator) ---- */
struct ida { struct xarray xa; };
#define DEFINE_IDA(name) struct ida name = { .xa = { 0 } }
static inline void ida_init(struct ida *ida) { xa_init_flags(&ida->xa, XA_FLAGS_ALLOC); }
int  ida_alloc(struct ida *ida, gfp_t gfp);
int  ida_alloc_range(struct ida *ida, unsigned int min, unsigned int max, gfp_t gfp);
void ida_free(struct ida *ida, unsigned int id);
void ida_destroy(struct ida *ida);

#endif /* _LINUXKPI_LINUX_IDR_H */
