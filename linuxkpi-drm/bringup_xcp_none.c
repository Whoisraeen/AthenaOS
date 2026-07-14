/* SPDX-License-Identifier: MPL-2.0 */
/*
 * RaeenOS' first supported accelerator is Phoenix 1002:15bf, which has no XCP
 * partition manager. Upstream amdgpu still references the XCP helpers from
 * generic open/reset/teardown code. This translation unit implements the exact
 * no-manager branches and rejects every partitioned-device operation instead
 * of allowing generated weak stubs to return register garbage.
 */
#include "amdgpu.h"

int amdgpu_xcp_query_partition_mode(struct amdgpu_xcp_mgr *mgr, u32 flags)
{
	(void)flags;
	return mgr ? -EOPNOTSUPP : AMDGPU_XCP_MODE_NONE;
}

int amdgpu_xcp_switch_partition_mode(struct amdgpu_xcp_mgr *mgr, int mode)
{
	(void)mode;
	return mgr ? -EOPNOTSUPP : -EINVAL;
}

int amdgpu_xcp_restore_partition_mode(struct amdgpu_xcp_mgr *mgr)
{
	return mgr ? -EOPNOTSUPP : 0;
}

int amdgpu_xcp_get_inst_details(struct amdgpu_xcp *xcp,
				enum AMDGPU_XCP_IP_BLOCK ip, u32 *inst_mask)
{
	if (!xcp || ip >= AMDGPU_XCP_MAX_BLOCKS || !xcp->valid ||
	    !inst_mask || !xcp->ip[ip].valid)
		return -EINVAL;
	*inst_mask = xcp->ip[ip].inst_mask;
	return 0;
}

int amdgpu_xcp_open_device(struct amdgpu_device *adev,
			   struct amdgpu_fpriv *fpriv,
			   struct drm_file *file_priv)
{
	(void)file_priv;
	if (!adev || !fpriv)
		return -EINVAL;
	if (adev->xcp_mgr)
		return -EOPNOTSUPP;
	return 0;
}

void amdgpu_xcp_release_sched(struct amdgpu_device *adev,
			      struct amdgpu_ctx_entity *entity)
{
	(void)entity;
	WARN_ON(adev && adev->xcp_mgr);
}

int amdgpu_xcp_select_scheds(struct amdgpu_device *adev, u32 hw_ip,
			     u32 hw_prio, struct amdgpu_fpriv *fpriv,
			     unsigned int *num_scheds,
			     struct drm_gpu_scheduler ***scheds)
{
	(void)adev; (void)hw_ip; (void)hw_prio; (void)fpriv;
	(void)num_scheds; (void)scheds;
	return -EOPNOTSUPP;
}

int amdgpu_xcp_update_partition_sched_list(struct amdgpu_device *adev)
{
	return adev && !adev->xcp_mgr ? 0 : -EOPNOTSUPP;
}

void amdgpu_xcp_sysfs_init(struct amdgpu_device *adev)
{
	WARN_ON(adev && adev->xcp_mgr);
}

void amdgpu_xcp_sysfs_fini(struct amdgpu_device *adev)
{
	WARN_ON(adev && adev->xcp_mgr);
}

void amdgpu_xcp_dev_unplug(struct amdgpu_device *adev)
{
	WARN_ON(adev && adev->xcp_mgr);
}
