/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <uapi/linux/sched/types.h> shim (MPL-2.0, original work).
 *
 * fetch-source.sh only vendors drivers/gpu/drm/amd + include/drm — this uapi
 * header lives outside that subtree. drm/scheduler/sched_main.c includes it
 * but (verified against the vendored 7.0.12 source) uses none of its symbols
 * (struct sched_param, SCHED_FIFO/etc.) — RaeenOS has no POSIX scheduling
 * policy surface, so an empty stub is faithful, not a fake (nothing to fake).
 */
#ifndef _LINUXKPI_UAPI_LINUX_SCHED_TYPES_H
#define _LINUXKPI_UAPI_LINUX_SCHED_TYPES_H
#endif
