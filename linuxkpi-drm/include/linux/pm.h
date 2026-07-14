/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/pm.h> shim (MPL-2.0, original work).
 *
 * Power-management types. amdgpu embeds `struct dev_pm_domain vga_pm_domain` by
 * value and tracks `suspend_state_t`. The actual runtime-PM transitions are
 * backed by ath_linuxkpi at M4 (and S3 suspend/resume is a separate phase);
 * here it is the type + callback-struct surface for layout. License boundary
 * (../../README.md): API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_PM_H
#define _LINUXKPI_LINUX_PM_H

#include <linux/types.h>

struct device;

typedef struct pm_message { int event; } pm_message_t;
typedef int suspend_state_t;
#define PM_SUSPEND_ON      0
#define PM_SUSPEND_TO_IDLE 1
#define PM_SUSPEND_STANDBY 2
#define PM_SUSPEND_MEM     3

struct dev_pm_ops {
	int (*prepare)(struct device *dev);
	void (*complete)(struct device *dev);
	int (*suspend)(struct device *dev);
	int (*resume)(struct device *dev);
	int (*freeze)(struct device *dev);
	int (*thaw)(struct device *dev);
	int (*poweroff)(struct device *dev);
	int (*restore)(struct device *dev);
	int (*runtime_suspend)(struct device *dev);
	int (*runtime_resume)(struct device *dev);
	int (*runtime_idle)(struct device *dev);
};

struct dev_pm_domain {
	struct dev_pm_ops ops;
	int  (*activate)(struct device *dev);
	void (*sync)(struct device *dev);
	void (*dismiss)(struct device *dev);
};

/* runtime-PM control — backed by ath_linuxkpi (M4) */
int  pm_runtime_get_sync(struct device *dev);
int  pm_runtime_put_sync(struct device *dev);
int  pm_runtime_put_autosuspend(struct device *dev);
void pm_runtime_enable(struct device *dev);
void pm_runtime_disable(struct device *dev);

#endif /* _LINUXKPI_LINUX_PM_H */
