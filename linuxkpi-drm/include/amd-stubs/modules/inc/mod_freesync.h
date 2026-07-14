/* SPDX-License-Identifier: MPL-2.0 */
/*
 * amd-stubs/modules/inc/mod_freesync.h — FreeSync module STUB (MPL-2.0, original).
 *
 * amdgpu_mode.h includes this for VRR/FreeSync types embedded in CRTC/connector
 * state. The real one pulls the DC display graph. The MES path drives no display,
 * so the FreeSync types are OPAQUE here (SCOPE.md display-stub). See
 * ../../amdgpu_dm.h for the rationale; opaque-by-value keeps amdgpu_mode.h's
 * layout self-consistent without the DC dependency.
 */
#ifndef _LINUXKPI_AMDSTUB_MOD_FREESYNC_H
#define _LINUXKPI_AMDSTUB_MOD_FREESYNC_H

struct mod_freesync_config { unsigned int _stub[8]; };
struct mod_vrr_params      { unsigned int _stub[16]; };
struct mod_freesync        { void *_stub; };

#endif /* _LINUXKPI_AMDSTUB_MOD_FREESYNC_H */
