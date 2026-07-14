/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/vgaarb.h> shim (MPL-2.0, original work).
 *
 * VGA arbitration — coordinates which GPU owns the legacy VGA resources when
 * several are present. Athena is a single-APU system, so amdgpu's arbitration
 * calls are honest no-ops here (nothing to arbitrate); backed by raeen_linuxkpi at
 * M4 only if multi-GPU VGA routing is ever needed. License boundary: API surface.
 */
#ifndef _LINUXKPI_LINUX_VGAARB_H
#define _LINUXKPI_LINUX_VGAARB_H

#include <linux/types.h>

struct pci_dev;

#define VGA_RSRC_LEGACY_IO  0x01
#define VGA_RSRC_LEGACY_MEM 0x02
#define VGA_RSRC_NORMAL_IO  0x04
#define VGA_RSRC_NORMAL_MEM 0x08

static inline int  vga_client_register(struct pci_dev *pdev, void *cb) { (void)pdev; (void)cb; return 0; }
static inline void vga_set_legacy_decoding(struct pci_dev *pdev, unsigned int decodes) { (void)pdev; (void)decodes; }

#endif /* _LINUXKPI_LINUX_VGAARB_H */
