/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/pm_runtime.h> shim (MPL-2.0, original work).
 *
 * Runtime power management. amdgpu wraps register access + submit in
 * pm_runtime_get/put to keep the GPU powered while in use. Backed by
 * raeen_linuxkpi at M4 (a fake get that didn't actually power-up could let the
 * driver poke an unpowered block; SCOPE.md rule 9). The base get/put live in
 * pm.h; this adds the fuller runtime-PM surface amdgpu uses. License boundary
 * (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_PM_RUNTIME_H
#define _LINUXKPI_LINUX_PM_RUNTIME_H

#include <linux/types.h>
#include <linux/pm.h>

struct device;

/* backed by raeen_linuxkpi (M4) */
int  pm_runtime_get(struct device *dev);
int  pm_runtime_get_if_in_use(struct device *dev);
int  pm_runtime_get_if_active(struct device *dev);
void pm_runtime_get_noresume(struct device *dev);
int  pm_runtime_put(struct device *dev);
void pm_runtime_put_noidle(struct device *dev);
void pm_runtime_mark_last_busy(struct device *dev);
void pm_runtime_set_active(struct device *dev);
void pm_runtime_set_suspended(struct device *dev);
void pm_runtime_use_autosuspend(struct device *dev);
void pm_runtime_set_autosuspend_delay(struct device *dev, int delay);
void pm_runtime_allow(struct device *dev);
void pm_runtime_forbid(struct device *dev);
int  pm_runtime_resume_and_get(struct device *dev);
int  pm_runtime_suspended(struct device *dev);

#endif /* _LINUXKPI_LINUX_PM_RUNTIME_H */
