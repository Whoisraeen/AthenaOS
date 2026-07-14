/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/cc_platform.h> shim (MPL-2.0, original work).
 *
 * Confidential-computing platform attributes (AMD SEV/SEV-SNP, Intel TDX). amdgpu
 * consults cc_platform_has() to decide whether to force-decrypt its DMA buffers.
 * AthenaOS bring-up runs on bare metal with no memory encryption, so every
 * attribute is absent — a truthful constant false (SCOPE.md rule 9: a no-op that
 * lied "encrypted" would corrupt DMA). License boundary (../../README.md): API.
 */
#ifndef _LINUXKPI_LINUX_CC_PLATFORM_H
#define _LINUXKPI_LINUX_CC_PLATFORM_H

#include <linux/types.h>

enum cc_attr {
	CC_ATTR_MEM_ENCRYPT,
	CC_ATTR_HOST_MEM_ENCRYPT,
	CC_ATTR_GUEST_MEM_ENCRYPT,
	CC_ATTR_GUEST_STATE_ENCRYPT,
	CC_ATTR_GUEST_UNROLL_STRING_IO,
	CC_ATTR_GUEST_SEV_SNP,
	CC_ATTR_HOTPLUG_DISABLED,
};

static inline bool cc_platform_has(enum cc_attr attr) { (void)attr; return false; }

#endif /* _LINUXKPI_LINUX_CC_PLATFORM_H */
