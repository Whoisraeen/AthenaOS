/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/signal.h> shim (MPL-2.0, original work).
 *
 * Signal-number constants. The DRM scheduler checks a dying task's exit_code
 * against SIGKILL to decide whether to discard its still-queued jobs rather
 * than wait for them (drm_sched_entity_flush). RaeenOS has no POSIX signal
 * delivery yet — this is just the numeric vocabulary a real kernel header
 * would pull in, so the comparison compiles and behaves like upstream.
 * License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_SIGNAL_H
#define _LINUXKPI_LINUX_SIGNAL_H

#define SIGKILL 9

#endif
