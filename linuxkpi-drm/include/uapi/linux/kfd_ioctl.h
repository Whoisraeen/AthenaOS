/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <uapi/linux/kfd_ioctl.h> shim (MPL-2.0, original work).
 *
 * The KFD (compute/HSA) userspace ioctl ABI. In the kernel this is a UAPI header;
 * it was not in the fetched subtree. The MES bring-up subset does not drive the
 * KFD ioctl path (SCOPE.md), but several amd files include it for the version
 * constants + a couple of flag enums. Provide just those; add more as a real
 * compile demands (error-driven). License boundary (../../README.md): surface.
 */
#ifndef _LINUXKPI_UAPI_LINUX_KFD_IOCTL_H
#define _LINUXKPI_UAPI_LINUX_KFD_IOCTL_H

#include <linux/types.h>

#define KFD_IOCTL_MAJOR_VERSION 1
#define KFD_IOCTL_MINOR_VERSION 17

/* byte offsets within the KFD-remapped HDP-flush MMIO page (kernel ABI; used by
 * nbio to place the HDP read/write flush registers). */
#define KFD_MMIO_REMAP_HDP_MEM_FLUSH_CNTL 0
#define KFD_MMIO_REMAP_HDP_REG_FLUSH_CNTL 4

#endif /* _LINUXKPI_UAPI_LINUX_KFD_IOCTL_H */
