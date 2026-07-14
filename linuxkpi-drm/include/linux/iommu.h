/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/iommu.h> shim (MPL-2.0, original work).
 *
 * IOMMU domain query. amdgpu checks whether its device sits behind an
 * IDENTITY (passthrough) vs translated IOMMU domain to decide GART/DMA setup.
 * The query is backed by raeen_linuxkpi's IOMMU facade (P4) at M4 (it reports the
 * real domain the daemon's device is in); the type/consts are the layout surface.
 * License boundary (../../README.md): API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_IOMMU_H
#define _LINUXKPI_LINUX_IOMMU_H

#include <linux/types.h>

struct device;

#define IOMMU_DOMAIN_BLOCKED    0x0
#define IOMMU_DOMAIN_IDENTITY   0x1
#define IOMMU_DOMAIN_UNMANAGED  0x2
#define IOMMU_DOMAIN_DMA        0x3

struct iommu_domain {
	unsigned int type;
	void        *priv;
};

/* domain query + map — backed by raeen_linuxkpi's IOMMU facade (M4) */
struct iommu_domain *iommu_get_domain_for_dev(struct device *dev);
bool iommu_present(const void *bus);
int  iommu_map(struct iommu_domain *domain, unsigned long iova, phys_addr_t paddr, size_t size, int prot, gfp_t gfp);
size_t iommu_unmap(struct iommu_domain *domain, unsigned long iova, size_t size);

#endif /* _LINUXKPI_LINUX_IOMMU_H */
