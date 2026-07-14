/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/device.h> shim (MPL-2.0, original work).
 *
 * The kernel driver/device model. amdgpu hangs everything off `struct device`
 * (the PCI device's embedded `dev`): dev_err/dev_warn diagnostics, dev_drvdata
 * for its adev back-pointer, and devm_* managed allocations tied to the device
 * lifetime. The struct carries the members amdgpu actually reaches; the
 * registration, logging, and managed-alloc machinery is backed by raeen_linuxkpi
 * at M4 (a no-op devm_kzalloc returning NULL, or a dev_err that dropped the
 * message, would both be silent failures — SCOPE.md rule 9). License boundary
 * (../../README.md): API surface only.
 */
#ifndef _LINUXKPI_LINUX_DEVICE_H
#define _LINUXKPI_LINUX_DEVICE_H

#include <linux/types.h>
#include <linux/compiler.h>
#include <linux/printk.h>
#include <linux/sysfs.h>   /* struct attribute, embedded by value below */
#include <linux/kobject.h> /* struct kobject — amdgpu embeds it by value */
#include <linux/pm.h>      /* struct dev_pm_domain — amdgpu embeds it by value */

struct device_driver;
struct bus_type;
struct device_type;
struct fwnode_handle;
struct device_node;

struct device {
	struct kobject         kobj;        /* embedded sysfs object (amdgpu reads dev->kobj) */
	dev_t                  devt;        /* DRM minor identity / diagnostics */
	struct device         *parent;
	const char            *init_name;
	const struct device_type *type;
	struct bus_type       *bus;
	struct device_driver  *driver;
	void                  *driver_data;   /* dev_get/set_drvdata */
	void                  *platform_data;
	u64                   *dma_mask;
	u64                    coherent_dma_mask;
	struct fwnode_handle  *fwnode;
	int                    numa_node;
	void (*release)(struct device *dev);
};

/* sysfs device attribute (embedded by value in amdgpu RAS/ACA/clock state). */
struct device_attribute {
	struct attribute attr;
	ssize_t (*show)(struct device *dev, struct device_attribute *attr, char *buf);
	ssize_t (*store)(struct device *dev, struct device_attribute *attr, const char *buf, size_t count);
};
#define DEVICE_ATTR(_name, _mode, _show, _store) \
	struct device_attribute dev_attr_##_name = __ATTR(_name, _mode, _show, _store)
#define DEVICE_ATTR_RO(_name) struct device_attribute dev_attr_##_name = __ATTR_RO(_name)
#define DEVICE_ATTR_RW(_name) struct device_attribute dev_attr_##_name = __ATTR_RW(_name)
int  device_create_file(struct device *dev, const struct device_attribute *attr);
void device_remove_file(struct device *dev, const struct device_attribute *attr);

/* drvdata accessors (pure). */
static inline void *dev_get_drvdata(const struct device *dev) { return dev->driver_data; }
static inline void  dev_set_drvdata(struct device *dev, void *data) { dev->driver_data = data; }

/* name: registered through raeen_linuxkpi (M4). */
const char *dev_name(const struct device *dev);
int dev_set_name(struct device *dev, const char *fmt, ...) __printf(2, 3);

/* logging: real emit through the M4 log facade (carries the dev prefix). */
__printf(3, 4) void _dev_printk(const char *level, const struct device *dev, const char *fmt, ...);
#define dev_emerg(dev, fmt, ...)  _dev_printk(KERN_EMERG,   dev, fmt, ##__VA_ARGS__)
#define dev_crit(dev, fmt, ...)   _dev_printk(KERN_CRIT,    dev, fmt, ##__VA_ARGS__)
#define dev_alert(dev, fmt, ...)  _dev_printk(KERN_ALERT,   dev, fmt, ##__VA_ARGS__)
#define dev_err(dev, fmt, ...)    _dev_printk(KERN_ERR,     dev, fmt, ##__VA_ARGS__)
#define dev_warn(dev, fmt, ...)   _dev_printk(KERN_WARNING, dev, fmt, ##__VA_ARGS__)
#define dev_notice(dev, fmt, ...) _dev_printk(KERN_NOTICE,  dev, fmt, ##__VA_ARGS__)
#define dev_info(dev, fmt, ...)   _dev_printk(KERN_INFO,    dev, fmt, ##__VA_ARGS__)
#define dev_dbg(dev, fmt, ...)    no_printk(fmt, ##__VA_ARGS__)
#define dev_err_once(dev, fmt, ...)        dev_err(dev, fmt, ##__VA_ARGS__)
#define dev_warn_once(dev, fmt, ...)       dev_warn(dev, fmt, ##__VA_ARGS__)
#define dev_err_ratelimited(dev, fmt, ...) dev_err(dev, fmt, ##__VA_ARGS__)
#define dev_warn_ratelimited(dev, fmt, ...) dev_warn(dev, fmt, ##__VA_ARGS__)
#define dev_info_ratelimited(dev, fmt, ...) dev_info(dev, fmt, ##__VA_ARGS__)
#define dev_WARN(dev, fmt, ...)   dev_warn(dev, fmt, ##__VA_ARGS__)

/* lifetime (refcounted) — backed by raeen_linuxkpi (M4). */
struct device *get_device(struct device *dev);
void put_device(struct device *dev);

/* device-managed allocation — freed when the device is released (M4). */
void *devm_kzalloc(struct device *dev, size_t size, gfp_t gfp);
void *devm_kcalloc(struct device *dev, size_t n, size_t size, gfp_t gfp);
void *devm_kmalloc(struct device *dev, size_t size, gfp_t gfp);
void  devm_kfree(struct device *dev, const void *p);
char *devm_kstrdup(struct device *dev, const char *s, gfp_t gfp);

static inline int dev_to_node(struct device *dev) { return dev ? dev->numa_node : -1; }
static inline void set_dev_node(struct device *dev, int node) { if (dev) dev->numa_node = node; }

#endif /* _LINUXKPI_LINUX_DEVICE_H */
