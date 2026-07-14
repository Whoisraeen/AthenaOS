/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/shmem_fs.h> shim (MPL-2.0, original work).
 *
 * tmpfs/shmem backing for swappable pages. TTM's `ttm_tt` swaps BO backing store
 * to a shmem file under memory pressure — OUT of the MES bring-up subset (no
 * eviction during init). Declarations only; backed by raeen_linuxkpi at M4 (or
 * left as an off-path link stub). License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_SHMEM_FS_H
#define _LINUXKPI_LINUX_SHMEM_FS_H

#include <linux/fs.h>
#include <linux/mm.h>

struct folio;
struct vfsmount;

struct page *shmem_read_mapping_page_gfp(struct address_space *mapping, unsigned long index, gfp_t gfp);
static inline struct page *shmem_read_mapping_page(struct address_space *mapping, unsigned long index)
{
	return shmem_read_mapping_page_gfp(mapping, index, GFP_KERNEL);
}
struct folio *shmem_read_folio_gfp(struct address_space *mapping, pgoff_t index,
				   gfp_t gfp);
struct file *shmem_file_setup(const char *name, loff_t size, vma_flags_t flags);
struct file *shmem_file_setup_with_mnt(struct vfsmount *mnt, const char *name,
				       loff_t size, vma_flags_t flags);
void shmem_truncate_range(struct inode *inode, loff_t start, loff_t end);

#endif /* _LINUXKPI_LINUX_SHMEM_FS_H */
