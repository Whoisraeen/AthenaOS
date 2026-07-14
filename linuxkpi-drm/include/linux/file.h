/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/file.h> shim (MPL-2.0, original work).
 *
 * File-descriptor table helpers. amdgpu_cs uses these to hand sync-file / syncobj
 * fds across the command-submission boundary. The fd table is owned by the guest
 * process; the daemon-side calls are backed by ath_linuxkpi at M4 (mapped onto
 * the AthenaOS handle/IPC surface). License boundary (../../README.md): API.
 */
#ifndef _LINUXKPI_LINUX_FILE_H
#define _LINUXKPI_LINUX_FILE_H

#include <linux/fs.h>
#include <linux/cleanup.h>

/* fd-creation flags (amdgpu_cs hands out sync-file fds with O_CLOEXEC). */
#ifndef O_CLOEXEC
#define O_CLOEXEC 02000000
#endif

struct fd {
	struct file *file;
	unsigned int flags;
};

/* fd <-> struct file — backed by ath_linuxkpi (M4) */
struct file *fget(unsigned int fd);
void         fput(struct file *file);
int          get_unused_fd_flags(unsigned int flags);
void         put_unused_fd(unsigned int fd);
void         fd_install(unsigned int fd, struct file *file);
struct fd    fdget(unsigned int fd);
void         fdput(struct fd fd);

DEFINE_CLASS(fd, struct fd, fdput(_T), fdget(fd), int fd)

static inline struct file *fd_file(struct fd f) { return f.file; }

#endif /* _LINUXKPI_LINUX_FILE_H */
