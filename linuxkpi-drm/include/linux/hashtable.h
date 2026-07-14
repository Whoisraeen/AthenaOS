/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/hashtable.h> shim (MPL-2.0, original work).
 *
 * Fixed-bucket hash table over hlist. amdgpu uses it for VM page-table BO maps
 * and MES queue->doorbell maps. REAL implementation: pure hlist surgery plus the
 * standard golden-ratio multiplicative bucket hash (the documented algorithm the
 * names carry). License boundary (../../README.md): API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_HASHTABLE_H
#define _LINUXKPI_LINUX_HASHTABLE_H

#include <linux/types.h>
#include <linux/list.h>

#ifndef ARRAY_SIZE
#define ARRAY_SIZE(arr) (sizeof(arr) / sizeof((arr)[0]))
#endif
#ifndef ilog2
#define ilog2(n) ((unsigned)(8 * sizeof(unsigned long long) - __builtin_clzll((unsigned long long)(n)) - 1))
#endif

#define GOLDEN_RATIO_32 0x61C88647U
#define GOLDEN_RATIO_64 0x61C8864680B583EBULL

static inline u32 hash_32(u32 val, unsigned int bits) { return (val * GOLDEN_RATIO_32) >> (32 - bits); }
static inline u32 hash_64(u64 val, unsigned int bits) { return (u32)((val * GOLDEN_RATIO_64) >> (64 - bits)); }
#define hash_long(val, bits) hash_64((u64)(val), (bits))
#define hash_min(val, bits) \
	(sizeof(val) <= 4 ? hash_32((u32)(val), (bits)) : hash_64((u64)(val), (bits)))

#define DEFINE_HASHTABLE(name, bits) \
	struct hlist_head name[1 << (bits)] = \
		{ [0 ... ((1 << (bits)) - 1)] = HLIST_HEAD_INIT }
#define DECLARE_HASHTABLE(name, bits) \
	struct hlist_head name[1 << (bits)]

#define HASH_SIZE(name) (ARRAY_SIZE(name))
#define HASH_BITS(name) ilog2(HASH_SIZE(name))

static inline void __hash_init(struct hlist_head *ht, unsigned int sz)
{ unsigned int i; for (i = 0; i < sz; i++) INIT_HLIST_NODE((struct hlist_node *)&ht[i].first), ht[i].first = (void *)0; }
#define hash_init(ht) __hash_init(ht, HASH_SIZE(ht))

#define hash_add(ht, node, key) \
	hlist_add_head(node, &(ht)[hash_min(key, HASH_BITS(ht))])
#define hash_del(node) hlist_del_init(node)
#define hash_empty(ht) __hash_empty(ht, HASH_SIZE(ht))
static inline bool __hash_empty(struct hlist_head *ht, unsigned int sz)
{ unsigned int i; for (i = 0; i < sz; i++) if (!hlist_empty(&ht[i])) return false; return true; }

#define hash_for_each(ht, bkt, obj, member) \
	for ((bkt) = 0; (bkt) < HASH_SIZE(ht); (bkt)++) \
		hlist_for_each_entry(obj, &(ht)[bkt], member)
#define hash_for_each_possible(ht, obj, member, key) \
	hlist_for_each_entry(obj, &(ht)[hash_min(key, HASH_BITS(ht))], member)
/* deletion-safe walk (amdgpu_sync clears its fence table this way). */
#define hash_for_each_safe(ht, bkt, tmp, obj, member) \
	for ((bkt) = 0; (bkt) < HASH_SIZE(ht); (bkt)++) \
		hlist_for_each_entry_safe(obj, tmp, &(ht)[bkt], member)
#define hash_for_each_possible_safe(ht, obj, tmp, member, key) \
	hlist_for_each_entry_safe(obj, tmp, &(ht)[hash_min(key, HASH_BITS(ht))], member)

#endif /* _LINUXKPI_LINUX_HASHTABLE_H */
