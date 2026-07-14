/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/pci-p2pdma.h> shim (MPL-2.0, original work).
 *
 * PCI peer-to-peer DMA (GPU<->GPU / GPU<->NVMe direct transfers). Out of the MES
 * bring-up subset (SCOPE.md). Backed by ath_linuxkpi at M4 if P2P is brought
 * into scope; the queries honestly report "not available". License boundary: surface.
 */
#ifndef _LINUXKPI_LINUX_PCI_P2PDMA_H
#define _LINUXKPI_LINUX_PCI_P2PDMA_H

#include <linux/types.h>

struct pci_dev;
struct scatterlist;

static inline int pci_p2pdma_distance(struct pci_dev *provider, struct device *client, bool verbose)
{ (void)provider; (void)client; (void)verbose; return -1; }
static inline bool pci_p2pdma_supported(struct pci_dev *pdev) { (void)pdev; return false; }

#endif /* _LINUXKPI_LINUX_PCI_P2PDMA_H */
