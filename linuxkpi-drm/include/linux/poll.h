/* SPDX-License-Identifier: MPL-2.0 */
/* Linux poll vocabulary used by the DRM file/event path.  The RaeenOS VFS
 * broker owns descriptor wake registration; drm_poll still reports the exact
 * readiness mask for already queued events. */
#ifndef _LINUXKPI_LINUX_POLL_H
#define _LINUXKPI_LINUX_POLL_H

#include <linux/fs.h>
#include <linux/wait.h>

#define EPOLLIN     0x00000001U
#define EPOLLRDNORM 0x00000040U
#define POLLIN      EPOLLIN
#define POLLRDNORM  EPOLLRDNORM

typedef struct poll_table_struct {
	unsigned int key;
} poll_table;

static inline void poll_wait(struct file *file, wait_queue_head_t *wait,
			     poll_table *table)
{
	(void)file;
	(void)wait;
	(void)table;
}

#endif /* _LINUXKPI_LINUX_POLL_H */
