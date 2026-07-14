/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/vga_switcheroo.h> shim (MPL-2.0, original work).
 *
 * Hybrid-graphics GPU switching (discrete<->integrated muxing on laptops).
 * Athena is a single-APU system with nothing to switch to, so amdgpu's
 * switcheroo registration is an honest no-op; backed by ath_linuxkpi at M4 only
 * if hybrid switching is ever needed. License boundary (../../README.md): surface.
 */
#ifndef _LINUXKPI_LINUX_VGA_SWITCHEROO_H
#define _LINUXKPI_LINUX_VGA_SWITCHEROO_H

#include <linux/types.h>

struct pci_dev;

enum vga_switcheroo_state { VGA_SWITCHEROO_OFF, VGA_SWITCHEROO_ON };
enum vga_switcheroo_client_id { VGA_SWITCHEROO_UNKNOWN_ID = -1, VGA_SWITCHEROO_IGD, VGA_SWITCHEROO_DIS };

struct vga_switcheroo_client_ops {
	void (*set_gpu_state)(struct pci_dev *dev, enum vga_switcheroo_state state);
	void (*reprobe)(struct pci_dev *dev);
	bool (*can_switch)(struct pci_dev *dev);
	bool (*gpu_bound)(struct pci_dev *dev, enum vga_switcheroo_client_id id);
};

static inline int vga_switcheroo_register_client(struct pci_dev *dev,
		const struct vga_switcheroo_client_ops *ops, bool driver_power_control)
{ (void)dev; (void)ops; (void)driver_power_control; return 0; }
static inline void vga_switcheroo_unregister_client(struct pci_dev *dev) { (void)dev; }
static inline int vga_switcheroo_process_delayed_switch(void) { return 0; }

#endif /* _LINUXKPI_LINUX_VGA_SWITCHEROO_H */
