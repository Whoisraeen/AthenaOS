/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/cgroup_dmem.h> shim (MPL-2.0, original work).
 *
 * Device-memory cgroup accounting — charges VRAM/GTT allocations to a cgroup so a
 * container can be limited. AthenaOS has no dmem cgroup controller (AthGuard owns
 * resource policy), so charging always succeeds with "no limit" and eviction is
 * always permitted — the honest answer for an unconstrained bring-up, not a fake
 * (SCOPE.md rule 9). License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_CGROUP_DMEM_H
#define _LINUXKPI_LINUX_CGROUP_DMEM_H

#include <linux/types.h>

struct dmem_cgroup_region;
struct dmem_cgroup_pool_state;

static inline int dmem_cgroup_try_charge(struct dmem_cgroup_region *region, u64 size,
					 struct dmem_cgroup_pool_state **ret_pool,
					 struct dmem_cgroup_pool_state **ret_limit_pool)
{
	(void)region; (void)size;
	if (ret_pool)       *ret_pool = (void *)0;
	if (ret_limit_pool) *ret_limit_pool = (void *)0;
	return 0; /* charged; no limit */
}
static inline void dmem_cgroup_uncharge(struct dmem_cgroup_pool_state *pool, u64 size)
{ (void)pool; (void)size; }
static inline bool dmem_cgroup_state_evict_valuable(struct dmem_cgroup_pool_state *limit_pool,
						    struct dmem_cgroup_pool_state *test_pool,
						    bool ignore_low, bool *ret_hit_low)
{ (void)limit_pool; (void)test_pool; (void)ignore_low; if (ret_hit_low) *ret_hit_low = false; return true; }
static inline void dmem_cgroup_pool_state_put(struct dmem_cgroup_pool_state *pool)
{ (void)pool; }

#endif /* _LINUXKPI_LINUX_CGROUP_DMEM_H */
