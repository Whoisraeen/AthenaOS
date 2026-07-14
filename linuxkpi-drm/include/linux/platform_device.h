/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/platform_device.h> shim (MPL-2.0, original work).
 *
 * The platform (non-discoverable) bus device/driver model. amdgpu reaches it for
 * the SoC-integrated GPU variants and for a couple of helper types; the discrete/
 * APU PCI path (our target) uses the PCI bus instead. Types laid out for the
 * declarations amdgpu spells; the register/resource calls are backed by
 * raeen_linuxkpi at M4. License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_PLATFORM_DEVICE_H
#define _LINUXKPI_LINUX_PLATFORM_DEVICE_H

#include <linux/types.h>
#include <linux/device.h>
#include <linux/ioport.h>
#include <linux/mod_devicetable.h>

struct platform_device {
	const char       *name;
	int               id;
	struct device     dev;
	u32               num_resources;
	struct resource  *resource;
	const struct platform_device_id *id_entry;
	void             *driver_data;
};

struct platform_driver {
	int  (*probe)(struct platform_device *);
	int  (*remove)(struct platform_device *);
	void (*shutdown)(struct platform_device *);
	int  (*suspend)(struct platform_device *, pm_message_t state);
	int  (*resume)(struct platform_device *);
	struct device_driver driver;
	const struct platform_device_id *id_table;
};

/* resource / irq lookup — backed by raeen_linuxkpi (M4) */
struct resource *platform_get_resource(struct platform_device *dev, unsigned int type, unsigned int num);
struct resource *platform_get_resource_byname(struct platform_device *dev, unsigned int type, const char *name);
int  platform_get_irq(struct platform_device *dev, unsigned int num);
int  platform_get_irq_byname(struct platform_device *dev, const char *name);

/* registration — backed by raeen_linuxkpi (M4) */
int  platform_driver_register(struct platform_driver *drv);
void platform_driver_unregister(struct platform_driver *drv);
struct platform_device *platform_device_register_simple(const char *name, int id,
							const struct resource *res, unsigned int num);
void platform_device_unregister(struct platform_device *pdev);

static inline void *platform_get_drvdata(const struct platform_device *pdev)
{
	return pdev->driver_data;
}
static inline void platform_set_drvdata(struct platform_device *pdev, void *data)
{
	pdev->driver_data = data;
}
static inline struct platform_device *to_platform_device(struct device *dev)
{
	return container_of(dev, struct platform_device, dev);
}

#define module_platform_driver(__drv) \
	static int __init __drv##_init(void) { return platform_driver_register(&(__drv)); } \
	static void __exit __drv##_exit(void) { platform_driver_unregister(&(__drv)); }

#endif /* _LINUXKPI_LINUX_PLATFORM_DEVICE_H */
