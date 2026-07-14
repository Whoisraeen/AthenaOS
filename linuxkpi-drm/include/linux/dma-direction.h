/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/dma-direction.h> shim (MPL-2.0, original work).
 *
 * The DMA transfer-direction enum (kernel ABI values). amdgpu tags every DMA map
 * with one. License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_DMA_DIRECTION_H
#define _LINUXKPI_LINUX_DMA_DIRECTION_H

enum dma_data_direction {
	DMA_BIDIRECTIONAL = 0,
	DMA_TO_DEVICE     = 1,
	DMA_FROM_DEVICE   = 2,
	DMA_NONE          = 3,
};

#endif /* _LINUXKPI_LINUX_DMA_DIRECTION_H */
