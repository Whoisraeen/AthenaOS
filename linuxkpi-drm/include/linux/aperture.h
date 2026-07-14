/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/aperture.h> shim (MPL-2.0, original work).
 *
 * Aperture-ownership handoff — on Linux a real GPU driver evicts the firmware
 * framebuffer (efifb/vesafb) from the PCI aperture before taking over. In the
 * RaeenOS model the compositor/scanout handoff is handled elsewhere (no fbdev to
 * evict), so this reports success-of-nothing to do; the real removal, if needed,
 * is backed by raeen_linuxkpi at M5. License boundary (../../README.md): surface.
 */
#ifndef _LINUXKPI_LINUX_APERTURE_H
#define _LINUXKPI_LINUX_APERTURE_H

#include <linux/types.h>

struct pci_dev;

static inline int aperture_remove_conflicting_pci_devices(struct pci_dev *pdev, const char *name)
{ (void)pdev; (void)name; return 0; }
static inline int aperture_remove_conflicting_devices(resource_size_t base, resource_size_t size, const char *name)
{ (void)base; (void)size; (void)name; return 0; }

#endif /* _LINUXKPI_LINUX_APERTURE_H */
