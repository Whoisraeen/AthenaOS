/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/dma-mapping.h> shim (MPL-2.0, original work).
 *
 * The DMA-coherent allocation + streaming-map API. amdgpu allocates its ring/MQD/
 * firmware-staging buffers with dma_alloc_coherent and maps BO pages with
 * dma_map_*. Backed by ath_linuxkpi's zero-copy dma_alloc_coherent facade (P3)
 * at M4 — a fake that returned a bogus bus address would make the GPU DMA to the
 * wrong place (SCOPE.md rule 9). License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_DMA_MAPPING_H
#define _LINUXKPI_LINUX_DMA_MAPPING_H

#include <linux/types.h>
#include <linux/device.h>
#include <linux/dma-direction.h>

#define DMA_BIT_MASK(n) (((n) == 64) ? ~0ULL : ((1ULL << (n)) - 1))
#define DMA_MAPPING_ERROR (~(dma_addr_t)0)

/* dma map/alloc attributes (passed to the *_attrs variants). */
#define DMA_ATTR_WEAK_ORDERING       (1UL << 1)
#define DMA_ATTR_WRITE_COMBINE       (1UL << 2)
#define DMA_ATTR_NO_KERNEL_MAPPING   (1UL << 4)
#define DMA_ATTR_SKIP_CPU_SYNC       (1UL << 5)
#define DMA_ATTR_FORCE_CONTIGUOUS    (1UL << 6)
#define DMA_ATTR_ALLOC_SINGLE_PAGES  (1UL << 7)
#define DMA_ATTR_NO_WARN             (1UL << 8)
#define DMA_ATTR_PRIVILEGED          (1UL << 9)

/* coherent (consistent) allocations — backed by ath_linuxkpi (M4) */
void *dma_alloc_coherent(struct device *dev, size_t size, dma_addr_t *dma_handle, gfp_t gfp);
void  dma_free_coherent(struct device *dev, size_t size, void *cpu_addr, dma_addr_t dma_handle);

/* streaming maps — backed by ath_linuxkpi (M4) */
dma_addr_t dma_map_single(struct device *dev, void *ptr, size_t size, enum dma_data_direction dir);
void       dma_unmap_single(struct device *dev, dma_addr_t addr, size_t size, enum dma_data_direction dir);
dma_addr_t dma_map_page(struct device *dev, struct page *page, size_t offset, size_t size, enum dma_data_direction dir);
void       dma_unmap_page(struct device *dev, dma_addr_t addr, size_t size, enum dma_data_direction dir);
int        dma_mapping_error(struct device *dev, dma_addr_t dma_addr);

/* mask configuration — backed by ath_linuxkpi (M4) */
int  dma_set_mask(struct device *dev, u64 mask);
int  dma_set_coherent_mask(struct device *dev, u64 mask);
int  dma_set_mask_and_coherent(struct device *dev, u64 mask);
u64  dma_get_required_mask(struct device *dev);

#endif /* _LINUXKPI_LINUX_DMA_MAPPING_H */
