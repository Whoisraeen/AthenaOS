/* SPDX-License-Identifier: MPL-2.0 */
/* Anonymous-inode API surface.  RaeenOS render clients are brokered by the
 * kernel rather than represented by Linux VFS fds inside amdgpud.  The facade
 * therefore provides the exact Linux signatures and fails fd-export paths
 * closed until the broker installs the corresponding RaeenOS descriptor. */
#ifndef _LINUXKPI_LINUX_ANON_INODES_H
#define _LINUXKPI_LINUX_ANON_INODES_H

#include <linux/types.h>

struct file;
struct file_operations;
struct inode;

struct file *anon_inode_getfile(const char *name,
				const struct file_operations *fops,
				void *priv, int flags);
struct file *anon_inode_getfile_fmode(const char *name,
				      const struct file_operations *fops,
				      void *priv, int flags, fmode_t f_mode);
struct file *anon_inode_create_getfile(const char *name,
				       const struct file_operations *fops,
				       void *priv, int flags,
				       const struct inode *context_inode);
int anon_inode_getfd(const char *name, const struct file_operations *fops,
		     void *priv, int flags);
int anon_inode_create_getfd(const char *name,
			    const struct file_operations *fops,
			    void *priv, int flags,
			    const struct inode *context_inode);

#endif /* _LINUXKPI_LINUX_ANON_INODES_H */
