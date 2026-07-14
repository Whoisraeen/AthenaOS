/* SPDX-License-Identifier: MPL-2.0 */
/*
 * amd-stubs/amdgpu_dm_irq_params.h — display-IRQ-params STUB (MPL-2.0, original).
 *
 * amdgpu_mode.h embeds `struct dm_irq_params` by value in its CRTC state. The real
 * one (display/amdgpu_dm/amdgpu_dm_irq_params.h) pulls mod_vrr_params +
 * dc_stream_state + the whole DC graph. The MES path never touches a CRTC, so we
 * define it OPAQUE here (SCOPE.md display-stub). See amdgpu_dm.h for the rationale.
 */
#ifndef _LINUXKPI_AMDSTUB_AMDGPU_DM_IRQ_PARAMS_H
#define _LINUXKPI_AMDSTUB_AMDGPU_DM_IRQ_PARAMS_H

struct dm_irq_params {
	void *_stub_opaque[8];
};

#endif /* _LINUXKPI_AMDSTUB_AMDGPU_DM_IRQ_PARAMS_H */
