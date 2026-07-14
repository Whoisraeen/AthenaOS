/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/slab.h> shim (MPL-2.0, original work).
 *
 * The kernel heap allocator. amdgpu allocates virtually all of its state through
 * kzalloc/kmalloc/kmem_cache. Backed by raeen_linuxkpi's allocator (one of its
 * 488 exports) at M4 — never faked (a kzalloc returning NULL silently would crash
 * init; SCOPE.md rule 9). The GFP_* flags are the allocation-context vocabulary;
 * their real meaning is interpreted by the M4 allocator. kzalloc/kcalloc are thin
 * inlines over kmalloc + __GFP_ZERO. License boundary (../../README.md): surface.
 */
#ifndef _LINUXKPI_LINUX_SLAB_H
#define _LINUXKPI_LINUX_SLAB_H

#include <linux/types.h>
/* Real <linux/slab.h> transitively reaches min_t/max_t (via the mm/gfp header
 * chain we don't vendor); callers like drm/scheduler's sched_entity.c use them
 * without including <linux/math.h> directly, same as upstream. */
#include <linux/math.h>

/* GFP allocation-context flags (guarded; a later <linux/gfp.h> may also set them). */
#ifndef ___GFP_DEFINED
#define ___GFP_DEFINED
#define __GFP_ZERO        ((gfp_t)0x100u)
#define __GFP_NOWARN      ((gfp_t)0x200u)
#define __GFP_RETRY_MAYFAIL ((gfp_t)0x400u)
#define __GFP_HIGH        ((gfp_t)0x800u)
#define __GFP_DMA32       ((gfp_t)0x1000u)
#define __GFP_THISNODE      ((gfp_t)0x2000u)
#define __GFP_NORETRY       ((gfp_t)0x4000u)
#define __GFP_NOMEMALLOC    ((gfp_t)0x8000u)
#define __GFP_DIRECT_RECLAIM ((gfp_t)0x10000u)
#define __GFP_COMP          ((gfp_t)0x20000u)
#define __GFP_KSWAPD_RECLAIM ((gfp_t)0x40000u)
#define __GFP_MOVABLE       ((gfp_t)0x80000u)
#define GFP_KERNEL        ((gfp_t)0x01u)
#define GFP_ATOMIC        ((gfp_t)0x02u)
#define GFP_NOWAIT        ((gfp_t)0x04u)
#define GFP_DMA           ((gfp_t)0x08u)
#define GFP_DMA32         __GFP_DMA32
#define GFP_USER          ((gfp_t)0x10u)
#define GFP_HIGHUSER      ((gfp_t)0x20u)
#define GFP_KERNEL_ACCOUNT GFP_KERNEL
#endif

/* core allocator — backed by raeen_linuxkpi (M4) */
void *kmalloc(size_t size, gfp_t flags);
void *kmalloc_array(size_t n, size_t size, gfp_t flags);
void *krealloc(const void *p, size_t new_size, gfp_t flags);
void  kfree(const void *objp);
void  kfree_sensitive(const void *objp);
size_t ksize(const void *objp);
char *kstrdup(const char *s, gfp_t gfp);
char *kstrndup(const char *s, size_t max, gfp_t gfp);
void *kmemdup(const void *src, size_t len, gfp_t gfp);

/* drm_managed.c uses the caller-tracking/NUMA and constant-string allocator
 * variants. RaeenOS has one kernel heap and no NUMA-local slab selection yet,
 * so preserve the allocation and ownership semantics while deliberately
 * ignoring only those Linux diagnostics/placement hints. */
#ifndef ARCH_DMA_MINALIGN
#define ARCH_DMA_MINALIGN 64
#endif
static inline void *kmalloc_node_track_caller(size_t size, gfp_t flags, int node)
{
	(void)node;
	return kmalloc(size, flags);
}
static inline char *kstrdup_const(const char *s, gfp_t flags)
{
	return kstrdup(s, flags);
}
static inline void kfree_const(const void *p)
{
	kfree(p);
}

/* zeroing/array convenience — inline over the above. */
static inline void *kzalloc(size_t size, gfp_t flags)            { return kmalloc(size, flags | __GFP_ZERO); }
/* kzalloc_obj: one zeroed object the size of the first arg (a type or an lvalue),
 * with an OPTIONAL gfp (defaults GFP_KERNEL) — both forms are used in-tree
 * (kzalloc_obj(struct X) and kzalloc_obj(*ptr, GFP_ATOMIC)). Arg-count overload. */
#define ___kzo_2(p, gfp) kzalloc(sizeof(p), (gfp))
#define ___kzo_1(p)      kzalloc(sizeof(p), GFP_KERNEL)
#define ___kzo_pick(_1, _2, NAME, ...) NAME
#define kzalloc_obj(...) ___kzo_pick(__VA_ARGS__, ___kzo_2, ___kzo_1)(__VA_ARGS__)
/* kmalloc_obj: same arg-count overload, non-zeroing (kmalloc). */
#define ___kmo_2(p, gfp) kmalloc(sizeof(p), (gfp))
#define ___kmo_1(p)      kmalloc(sizeof(p), GFP_KERNEL)
#define kmalloc_obj(...) ___kzo_pick(__VA_ARGS__, ___kmo_2, ___kmo_1)(__VA_ARGS__)
#define kzalloc_objs(type, n) kzalloc(sizeof(type) * (size_t)(n), GFP_KERNEL)
#define kcalloc_objs(type, n) kzalloc_objs(type, n)
/* kvzalloc/kvmalloc array variants (fall back to vmalloc for large allocs);
 * kvmalloc/kvzalloc are declared in <linux/mm.h>, which the call sites include. */
#define kvzalloc_objs(type, n) kvcalloc((size_t)(n), sizeof(type), GFP_KERNEL)
/* Linux 7 accepts an optional explicit GFP argument.  Route both forms through
 * kvmalloc_array so a hostile GEM handle count cannot wrap the allocation. */
#define ___kvmobjs_3(p, n, gfp) kvmalloc_array((size_t)(n), sizeof(p), (gfp))
#define ___kvmobjs_2(p, n)      kvmalloc_array((size_t)(n), sizeof(p), GFP_KERNEL)
#define kvmalloc_objs(...) \
	___kmos_pick(__VA_ARGS__, ___kvmobjs_3, ___kvmobjs_2)(__VA_ARGS__)
/* kmalloc_objs: array alloc, same arg-count overload as kzalloc_obj — the
 * 2-arg form (type/lvalue, count) defaults GFP_KERNEL (every existing in-tree
 * caller); drm/scheduler's sched_main.c is the one 3-arg caller, passing an
 * explicit flags (GFP_KERNEL | __GFP_ZERO). */
#define ___kmos_3(p, n, gfp) kmalloc(sizeof(p) * (size_t)(n), (gfp))
#define ___kmos_2(p, n)      kmalloc(sizeof(p) * (size_t)(n), GFP_KERNEL)
#define ___kmos_pick(_1, _2, _3, NAME, ...) NAME
#define kmalloc_objs(...) ___kmos_pick(__VA_ARGS__, ___kmos_3, ___kmos_2)(__VA_ARGS__)
/* kvzalloc_obj: single zeroed object via kvmalloc path, optional gfp (overload). */
#define ___kvzo_2(p, gfp) kvzalloc(sizeof(p), (gfp))
#define ___kvzo_1(p)      kvzalloc(sizeof(p), GFP_KERNEL)
#define kvzalloc_obj(...) ___kzo_pick(__VA_ARGS__, ___kvzo_2, ___kvzo_1)(__VA_ARGS__)
/* kzalloc_flex: one zeroed object plus a trailing flex array. Same dereferenced-
 * lvalue convention as kzalloc_obj — arg 1 is the *object* expression (e.g.
 * *ip_hw_instance), arg 2 the flex MEMBER name (a member token, never evaluated),
 * arg 3 the element count. Size = struct + count*sizeof(elem). */
#define struct_size_flex(P, member, count) \
	(sizeof(P) + (size_t)(count) * sizeof((P).member[0]))
#define kzalloc_flex(P, member, count) \
	kzalloc(struct_size_flex(P, member, count), GFP_KERNEL)
#define kvzalloc_flex(P, member, count) \
	kvzalloc(struct_size_flex(P, member, count), GFP_KERNEL)
static inline void *kcalloc(size_t n, size_t size, gfp_t flags)  { return kmalloc_array(n, size, flags | __GFP_ZERO); }
static inline void *kzalloc_node(size_t size, gfp_t flags, int node) { (void)node; return kzalloc(size, flags); }

/* slab caches — backed by raeen_linuxkpi (M4) */
struct kmem_cache;
struct kmem_cache *kmem_cache_create(const char *name, unsigned int size, unsigned int align,
				     unsigned long flags, void (*ctor)(void *));
void  kmem_cache_destroy(struct kmem_cache *s);
void *kmem_cache_alloc(struct kmem_cache *s, gfp_t flags);
void *kmem_cache_zalloc(struct kmem_cache *s, gfp_t flags);
void  kmem_cache_free(struct kmem_cache *s, void *objp);

/* kmem_cache_create() flags */
#define SLAB_HWCACHE_ALIGN    0x00002000UL
#define SLAB_RECLAIM_ACCOUNT  0x00020000UL
#define SLAB_PANIC            0x00040000UL
#define SLAB_TYPESAFE_BY_RCU  0x00080000UL

/* create a cache named after the struct, sized + aligned to it (the common form). */
#define KMEM_CACHE(__struct, __flags) \
	kmem_cache_create(#__struct, sizeof(struct __struct), \
			  __alignof__(struct __struct), (__flags), (void *)0)

#endif /* _LINUXKPI_LINUX_SLAB_H */
