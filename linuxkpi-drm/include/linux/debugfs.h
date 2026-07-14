/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/debugfs.h> shim (MPL-2.0, original work).
 *
 * Debug filesystem. amdgpu exposes RAS/ring/firmware diagnostics through it. Not
 * present in the AthenaOS model (the CONFIG_DEBUG_FS=n posture — a legitimate
 * config, not a fake): create returns NULL and remove is a no-op. amdgpu treats a
 * NULL debugfs handle as "unavailable" and skips, so no functional path breaks;
 * the real diagnostic surface is /proc/raeen instead. License boundary: surface.
 */
#ifndef _LINUXKPI_LINUX_DEBUGFS_H
#define _LINUXKPI_LINUX_DEBUGFS_H

#include <linux/types.h>

struct dentry;
struct file_operations;
struct debugfs_blob_wrapper { void *data; unsigned long size; };

static inline struct dentry *debugfs_create_dir(const char *name, struct dentry *parent)
{ (void)name; (void)parent; return (struct dentry *)0; }
static inline struct dentry *debugfs_create_file(const char *name, umode_t mode, struct dentry *parent,
						 void *data, const struct file_operations *fops)
{ (void)name; (void)mode; (void)parent; (void)data; (void)fops; return (struct dentry *)0; }
static inline void debugfs_remove(struct dentry *dentry) { (void)dentry; }
static inline void debugfs_remove_recursive(struct dentry *dentry) { (void)dentry; }
static inline bool debugfs_initialized(void) { return false; }

static inline void debugfs_create_u32(const char *n, umode_t m, struct dentry *p, u32 *v)  { (void)n; (void)m; (void)p; (void)v; }
static inline void debugfs_create_u64(const char *n, umode_t m, struct dentry *p, u64 *v)  { (void)n; (void)m; (void)p; (void)v; }
static inline void debugfs_create_bool(const char *n, umode_t m, struct dentry *p, bool *v){ (void)n; (void)m; (void)p; (void)v; }
static inline void debugfs_create_x32(const char *n, umode_t m, struct dentry *p, u32 *v)  { (void)n; (void)m; (void)p; (void)v; }
static inline void debugfs_create_x64(const char *n, umode_t m, struct dentry *p, u64 *v)  { (void)n; (void)m; (void)p; (void)v; }

#endif /* _LINUXKPI_LINUX_DEBUGFS_H */
