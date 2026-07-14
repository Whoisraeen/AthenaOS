/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/kobject.h> shim (MPL-2.0, original work).
 *
 * The sysfs object base. amdgpu embeds `struct kobject` BY VALUE in its xcp/ras
 * sysfs nodes, so the type must be fully defined for layout. The refcount is a
 * real kref; the sysfs registration (kobject_add/init_and_add) is backed by
 * ath_linuxkpi at M4. License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_KOBJECT_H
#define _LINUXKPI_LINUX_KOBJECT_H

#include <linux/types.h>
#include <linux/kref.h>

struct kobject;
struct kobj_type;
struct kset;
struct attribute;

struct kernfs_node;

struct kobject {
	const char       *name;
	struct list_head  entry;
	struct kobject   *parent;
	struct kset      *kset;
	const struct kobj_type *ktype;
	struct kernfs_node *sd;       /* sysfs directory entry (amdgpu uses kobj->sd) */
	struct kref       kref;
	unsigned int      state_initialized : 1;
	unsigned int      state_in_sysfs : 1;
};

struct kobj_type {
	void (*release)(struct kobject *kobj);
	const struct sysfs_ops *sysfs_ops;
	const struct attribute_group **default_groups;
};

struct sysfs_ops {
	ssize_t (*show)(struct kobject *, struct attribute *, char *);
	ssize_t (*store)(struct kobject *, struct attribute *, const char *, size_t);
};

/* the default kobject sysfs ops the kernel exports (amdgpu's IP-discovery ksets
 * point their ktype at it). Backed by ath_linuxkpi at M4. */
extern const struct sysfs_ops kobj_sysfs_ops;

struct kset_uevent_ops;
struct kset {
	struct list_head  list;       /* member kobjects */
	spinlock_t        list_lock;
	struct kobject    kobj;        /* embedded by value (amdgpu embeds ksets) */
	const struct kset_uevent_ops *uevent_ops;
};

/* kset registration — backed by ath_linuxkpi (M4) */
struct kset *kset_create_and_add(const char *name, const struct kset_uevent_ops *u, struct kobject *parent);
int  kset_register(struct kset *kset);
void kset_unregister(struct kset *kset);

/* registration / lifetime — backed by ath_linuxkpi (M4) */
void  kobject_init(struct kobject *kobj, const struct kobj_type *ktype);
int   kobject_add(struct kobject *kobj, struct kobject *parent, const char *fmt, ...);
int   kobject_init_and_add(struct kobject *kobj, const struct kobj_type *ktype,
			   struct kobject *parent, const char *fmt, ...);
void  kobject_del(struct kobject *kobj);
struct kobject *kobject_get(struct kobject *kobj);
void  kobject_put(struct kobject *kobj);
void  kobject_set_name(struct kobject *kobj, const char *fmt, ...);

#endif /* _LINUXKPI_LINUX_KOBJECT_H */
