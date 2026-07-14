/* SPDX-License-Identifier: MPL-2.0 */
/* eventfd surface used by the optional DRM_SYNCOBJ_EVENTFD ioctl.  Descriptor
 * lookup is broker-owned on RaeenOS and fails closed until fd translation is
 * installed. */
#ifndef _LINUXKPI_LINUX_EVENTFD_H
#define _LINUXKPI_LINUX_EVENTFD_H

#include <linux/types.h>

struct eventfd_ctx;

struct eventfd_ctx *eventfd_ctx_fdget(int fd);
void eventfd_ctx_put(struct eventfd_ctx *ctx);
void eventfd_signal_mask(struct eventfd_ctx *ctx, __poll_t mask);

static inline void eventfd_signal(struct eventfd_ctx *ctx)
{
	eventfd_signal_mask(ctx, 0);
}

#endif /* _LINUXKPI_LINUX_EVENTFD_H */
