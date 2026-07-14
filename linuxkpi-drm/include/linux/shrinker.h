/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/shrinker.h> shim (MPL-2.0, original work).
 *
 * The memory-reclaim shrinker callback registry. TTM's page pool registers a
 * shrinker so the kernel can reclaim cached free pages under memory pressure —
 * OUT of the MES bring-up subset (no reclaim during init; the daemon owns its
 * heap). The type is laid out so ttm_pool's shrinker can be defined by value;
 * register/free are backed by ath_linuxkpi as no-ops (nothing reclaims here).
 * License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_SHRINKER_H
#define _LINUXKPI_LINUX_SHRINKER_H

#include <linux/types.h>

/* shrinker scan/count return sentinels */
#define SHRINK_STOP  (~0UL)
#define SHRINK_EMPTY (~0UL - 1)

struct shrink_control {
	gfp_t         gfp_mask;
	int           nid;
	unsigned long nr_to_scan;
	unsigned long nr_scanned;
	void         *memcg;
};

struct shrinker {
	unsigned long (*count_objects)(struct shrinker *, struct shrink_control *sc);
	unsigned long (*scan_objects)(struct shrinker *, struct shrink_control *sc);
	long          batch;
	int           seeks;
	unsigned int  flags;
	void         *private_data;
};

/* allocation/registration — backed by ath_linuxkpi (M4). alloc returns a real
 * zeroed shrinker so the caller can set its callbacks; register/free are no-ops
 * (nothing drives reclaim during bring-up). */
struct shrinker *shrinker_alloc(unsigned int flags, const char *fmt, ...);
void shrinker_register(struct shrinker *shrinker);
void shrinker_free(struct shrinker *shrinker);

#endif /* _LINUXKPI_LINUX_SHRINKER_H */
