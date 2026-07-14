/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/sync_file.h> shim (MPL-2.0, original work).
 *
 * A sync_file wraps a dma_fence behind a file descriptor so userspace can wait on
 * GPU work across the command-submission boundary. amdgpu_cs hands these out as
 * the out-fence of a submit. Type laid out for the fd plumbing; create/get backed
 * by ath_linuxkpi at M4. License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_SYNC_FILE_H
#define _LINUXKPI_LINUX_SYNC_FILE_H

#include <linux/types.h>
#include <linux/dma-fence.h>
#include <linux/fs.h>
#include <linux/wait.h>   /* wait_queue_head_t */
#include <linux/list.h>

struct sync_file {
	struct file      *file;
	char              user_name[32];
	struct list_head  lock_list_node;
	struct dma_fence *fence;
	wait_queue_head_t wq;
	unsigned long     flags;
};

/* backed by ath_linuxkpi (M4) */
struct sync_file *sync_file_create(struct dma_fence *fence);
struct dma_fence *sync_file_get_fence(int fd);

#endif /* _LINUXKPI_LINUX_SYNC_FILE_H */
