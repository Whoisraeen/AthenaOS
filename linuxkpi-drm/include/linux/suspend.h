/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/suspend.h> shim (MPL-2.0, original work).
 *
 * System sleep-state surface. amdgpu_device queries the target suspend state
 * (S3 vs s2idle) to pick its reset/power path. S3 suspend/resume is a separate
 * AthenaOS phase; here it is the suspend_state_t + the query, backed by
 * raeen_linuxkpi at M4 (reports the real target). suspend_state_t lives in
 * <linux/pm.h>. License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_SUSPEND_H
#define _LINUXKPI_LINUX_SUSPEND_H

#include <linux/types.h>
#include <linux/pm.h>   /* suspend_state_t + PM_SUSPEND_* */

/* PM notifier event codes (amdgpu registers a pm_notifier for hibernate/suspend). */
#define PM_HIBERNATION_PREPARE  1
#define PM_POST_HIBERNATION     2
#define PM_SUSPEND_PREPARE      3
#define PM_POST_SUSPEND         4
#define PM_RESTORE_PREPARE      5
#define PM_POST_RESTORE         6

/* the in-progress system sleep target -- backed by raeen_linuxkpi (M4) */
suspend_state_t pm_suspend_target_state(void);
bool pm_suspend_via_firmware(void);
bool pm_resume_via_firmware(void);
bool pm_suspend_no_platform(void);

#endif /* _LINUXKPI_LINUX_SUSPEND_H */
