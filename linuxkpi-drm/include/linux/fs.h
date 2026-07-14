/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/fs.h> shim (MPL-2.0, original work).
 *
 * VFS inode/file surface. amdgpu touches `struct inode`/`struct file` only for its
 * char-device file_operations + the BO-mmap path (peripheral to MES bring-up).
 * Minimal definitions for the type layout; the file ops are driven by the guest
 * IPC/capability path, backed by ath_linuxkpi at M4. License boundary: surface.
 */
#ifndef _LINUXKPI_LINUX_FS_H
#define _LINUXKPI_LINUX_FS_H

#include <linux/types.h>

struct address_space;
struct module;
struct vm_area_struct;
struct poll_table_struct;
struct seq_file;

/* Linux open flags consumed by the DRM UAPI and PRIME fd validation. */
#ifndef O_RDONLY
#define O_RDONLY   00000000
#define O_WRONLY   00000001
#define O_RDWR     00000002
#define O_CLOEXEC  02000000
#define O_EXCL     00000200
#define O_NONBLOCK 00004000
#endif

struct inode {
	umode_t              i_mode;
	unsigned long        i_ino;
	loff_t               i_size;
	struct address_space *i_mapping;
	void                *i_private;
	dev_t                i_rdev;
};

/* Page-cache mapping. TTM stores this as bdev->dev_mapping (from the drm_device's
 * anon_inode) and only walks its fields on BO eviction / unmap_mapping_range —
 * never during the headless MES/GFX init. A complete-but-minimal layout lets the
 * bring-up allocate one so ttm_device_init's mapping arg is deterministic. */
struct address_space {
	struct inode *host;
	unsigned long nrpages;
	unsigned long flags;
	void         *private_data;
};

struct file {
	void                       *private_data;
	struct inode               *f_inode;
	const struct file_operations *f_op;
	unsigned int                f_flags;
	loff_t                      f_pos;
	struct address_space       *f_mapping;   /* the file's page cache (TTM swap) */
};

/* char-device / debugfs callback table. amdgpu defines several of these by value
 * (RAS debugfs nodes); the layout must be complete. The ops themselves run over
 * the guest IPC/capability path (backed by ath_linuxkpi at M4). */
struct file_operations {
	struct module *owner;
	unsigned int fop_flags;
	loff_t  (*llseek)(struct file *, loff_t, int);
	ssize_t (*read)(struct file *, char __user *, size_t, loff_t *);
	ssize_t (*write)(struct file *, const char __user *, size_t, loff_t *);
	long    (*unlocked_ioctl)(struct file *, unsigned int, unsigned long);
	long    (*compat_ioctl)(struct file *, unsigned int, unsigned long);
	int     (*mmap)(struct file *, struct vm_area_struct *);
	int     (*open)(struct inode *, struct file *);
	int     (*release)(struct inode *, struct file *);
	__poll_t (*poll)(struct file *, struct poll_table_struct *);
	int     (*flush)(struct file *, void *id);
	int     (*fsync)(struct file *, loff_t, loff_t, int);
};

#define FOP_UNSIGNED_OFFSET (1U << 5)

static inline struct inode *file_inode(const struct file *f) { return f->f_inode; }

/* llseek helpers used in file_operations initializers (backed by ath_linuxkpi). */
loff_t default_llseek(struct file *file, loff_t offset, int whence);
loff_t noop_llseek(struct file *file, loff_t offset, int whence);
loff_t generic_file_llseek(struct file *file, loff_t offset, int whence);
#define no_llseek NULL

/* mm-side helper used by the device teardown (backed by ath_linuxkpi, M4). */
void unmap_mapping_range(struct address_space *mapping, loff_t holebegin, loff_t holelen, int even_cows);

#endif /* _LINUXKPI_LINUX_FS_H */
