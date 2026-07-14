/* SPDX-License-Identifier: MPL-2.0 */
/*
 * amd-stubs/amdgpu_dm.h — SCOPE-aligned display-manager STUB (MPL-2.0, original).
 *
 * The real display core (DC/DM) is OUT of the MES bring-up subset (SCOPE.md:
 * "display ... #if 0'd or stubbed"). But amdgpu_device embeds `struct
 * amdgpu_display_manager dm` BY VALUE and UNGATED (amdgpu.h:1071), so the type
 * must EXIST for amdgpu_device to lay out — even though the MES path never
 * dereferences it (verified: no `->dm.`/amdgpu_dm_* in mes_v11_0.c / amdgpu_mes.c).
 *
 * So this shadow header (earlier on the -I path than display/amdgpu_dm/) defines
 * the manager as OPAQUE. This cuts the entire DC type graph (dc_*, link, hubp,
 * dpp, ... hundreds of headers) we would otherwise drag in for layout only. The
 * size differs from the real driver, but we compile our OWN amdgpu_device from
 * source consistently, so it is self-consistent for the MES subset. If the display
 * path is ever brought into scope, drop this stub and add -I display/amdgpu_dm.
 */
#ifndef _LINUXKPI_AMDSTUB_AMDGPU_DM_H
#define _LINUXKPI_AMDSTUB_AMDGPU_DM_H

/* Opaque: large enough to be a real allocation, never introspected by the MES
 * path. (Sized generously so a stray memset/zero of the embedding struct stays
 * in-bounds; exact layout is irrelevant since no compiled code reads its fields.) */
struct amdgpu_display_manager {
	/* amdgpu_device.c (the full device init, beyond the MES path) reads a few
	 * dm members by name even with DC stubbed; expose them as inert fields so it
	 * typechecks. The MES path still never derefs dm. */
	void *soc_bounding_box;
	/* amdgpu_ucode.c reports the display-microcontroller (DMCU/DMCUB) firmware
	 * versions; inert here (DC stubbed) but must exist by name. */
	unsigned int dmcu_fw_version;
	unsigned int dmcub_fw_version;
	void *_stub_opaque[64];
};

#endif /* _LINUXKPI_AMDSTUB_AMDGPU_DM_H */
