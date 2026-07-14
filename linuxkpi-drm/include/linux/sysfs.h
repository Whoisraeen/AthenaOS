/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/sysfs.h> shim (MPL-2.0, original work).
 *
 * sysfs attribute model. amdgpu exposes RAS/ACA/clock state as sysfs files and
 * embeds `struct device_attribute`/`struct attribute_group` BY VALUE in its
 * device state, so the types must be fully defined for layout. The create/remove
 * ops are backed by ath_linuxkpi at M4 (the daemon's introspection surface
 * stands in for sysfs). License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_SYSFS_H
#define _LINUXKPI_LINUX_SYSFS_H

#include <linux/types.h>
#include <linux/stat.h>   /* S_IWUSR etc. — attribute modes are spelled with these */

struct attribute {
	const char *name;
	umode_t     mode;
};

struct attribute_group {
	const char         *name;
	umode_t           (*is_visible)(struct kobject *, struct attribute *, int);
	umode_t           (*is_bin_visible)(struct kobject *, struct bin_attribute *, int);
	struct attribute  **attrs;
	struct bin_attribute **bin_attrs;
};

struct bin_attribute {
	struct attribute attr;
	size_t           size;
	void            *private;
	ssize_t (*read)(struct file *, struct kobject *, struct bin_attribute *, char *, loff_t, size_t);
	ssize_t (*write)(struct file *, struct kobject *, struct bin_attribute *, char *, loff_t, size_t);
};

struct kobject;
struct file;

#define __ATTR(_name, _mode, _show, _store) { \
	.attr = { .name = #_name, .mode = (_mode) }, .show = (_show), .store = (_store) }
#define __ATTR_RO(_name) { .attr = { .name = #_name, .mode = 0444 }, .show = _name##_show }
#define __ATTR_RW(_name) __ATTR(_name, 0644, _name##_show, _name##_store)
/* ATTRIBUTE_GROUPS must define BOTH `_name##_group` and the `_name##_groups[]`
 * NULL-terminated array (amdgpu references the singular form too). */
#define ATTRIBUTE_GROUPS(_name) \
	static const struct attribute_group _name##_group = { .attrs = _name##_attrs }; \
	static const struct attribute_group *_name##_groups[] = { &_name##_group, (void *)0 }

/* binary sysfs attribute (amdgpu exposes reg_state / vbios as a BIN_ATTR). */
#define BIN_ATTR(_name, _mode, _read, _write, _size) \
	struct bin_attribute bin_attr_##_name = { \
		.attr = { .name = #_name, .mode = (_mode) }, \
		.size = (_size), .read = (_read), .write = (_write) }
#define BIN_ATTR_RO(_name, _size) \
	struct bin_attribute bin_attr_##_name = { \
		.attr = { .name = #_name, .mode = 0444 }, .size = (_size), .read = _name##_read }
#define __BIN_ATTR_NULL { .attr = { .name = (void *)0 } }

/* lockdep-key initialisers for dynamically-allocated attributes — no-op without
 * lockdep (the attribute is still fully usable). */
#define sysfs_attr_init(attr)     do { } while (0)
#define sysfs_bin_attr_init(attr) do { } while (0)

/* create/remove — backed by ath_linuxkpi (M4) */
int  sysfs_create_file(struct kobject *kobj, const struct attribute *attr);
void sysfs_remove_file(struct kobject *kobj, const struct attribute *attr);
int  sysfs_create_group(struct kobject *kobj, const struct attribute_group *grp);
void sysfs_remove_group(struct kobject *kobj, const struct attribute_group *grp);
int  sysfs_create_bin_file(struct kobject *kobj, const struct bin_attribute *attr);
void sysfs_remove_bin_file(struct kobject *kobj, const struct bin_attribute *attr);

#endif /* _LINUXKPI_LINUX_SYSFS_H */
