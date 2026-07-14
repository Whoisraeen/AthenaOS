// SPDX-License-Identifier: MPL-2.0
/*
 * bringup_ip_noop.c — valid no-op IP-block descriptors (MPL-2.0, original work).
 *
 * Phoenix's amdgpu_discovery adds the multimedia IPs (VCN video-decode, JPEG,
 * UMSCH multimedia scheduler) to adev->ip_blocks. They are OUT of scope for the
 * 3D/gaming graphics bring-up (no video decode is needed to render), but
 * amdgpu_device_ip_init() dereferences each block's ->funcs->{early_init,sw_init,
 * hw_init} unconditionally — so a zeroed link stub (NULL .funcs) would fault init.
 *
 * These give those IPs a REAL descriptor whose hw_init succeeds and provides no
 * engine — the honest "present but inert" answer (SCOPE.md rule 9), letting init
 * walk past them to the GFX/GMC/MES path that matters. The struct types come from
 * the real (compiled-against, not copied) amdgpu.h; the descriptors are original.
 *
 * If video decode is ever brought into scope, drop these and compile the real
 * vcn_v4_0.c / jpeg_v4_0.c instead (they compile clean but pull the whole video
 * subsystem — see M5-ONPATH-AUDIT.md).
 */
#include "amdgpu.h"

static int rae_ip_noop(struct amdgpu_ip_block *ip_block)
{
	(void)ip_block;
	return 0;
}

static const struct amd_ip_funcs rae_noop_ip_funcs = {
	.name       = "rae_noop",
	.early_init = rae_ip_noop,
	.sw_init    = rae_ip_noop,
	.hw_init    = rae_ip_noop,
	.sw_fini    = rae_ip_noop,
	.hw_fini    = rae_ip_noop,
};

const struct amdgpu_ip_block_version vcn_v4_0_ip_block = {
	.type = AMD_IP_BLOCK_TYPE_VCN,  .major = 4, .minor = 0, .rev = 0, .funcs = &rae_noop_ip_funcs,
};
const struct amdgpu_ip_block_version jpeg_v4_0_ip_block = {
	.type = AMD_IP_BLOCK_TYPE_JPEG, .major = 4, .minor = 0, .rev = 0, .funcs = &rae_noop_ip_funcs,
};
const struct amdgpu_ip_block_version umsch_mm_v4_0_ip_block = {
	.type = AMD_IP_BLOCK_TYPE_UMSCH_MM, .major = 4, .minor = 0, .rev = 0, .funcs = &rae_noop_ip_funcs,
};
