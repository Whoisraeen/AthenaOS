/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/kfd_sysfs.h> shim (MPL-2.0, original work).
 *
 * KFD topology sysfs ABI constants. In the kernel this is a UAPI header
 * (include/uapi/linux/kfd_sysfs.h); it was not in the fetched subtree, so the few
 * symbols kfd_topology.h actually needs are provided here.
 *
 * The only consumers reached on the build path are the GPU debug-watch address
 * mask bit-positions below — used by KFD's hardware-debugger support, which is
 * OUT of the MES bring-up subset (SCOPE.md) and never executed by mes_v11_0.c.
 * They are PLACEHOLDER bit positions (self-consistent, but NOT verified against
 * the real UAPI). This is NOT a silent-success fake of a live path (rule 9): the
 * MES path runs none of this. If KFD hardware debugging is ever brought into
 * scope, replace these with the real include/uapi/linux/kfd_sysfs.h values.
 *
 * License boundary (../../README.md): API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_KFD_SYSFS_H
#define _LINUXKPI_LINUX_KFD_SYSFS_H

/* ---- GPU debug-watch address mask bit layout (OUT-OF-SUBSET; placeholder) ---- */
#define HSA_DBG_WATCH_ADDR_MASK_LO_BIT_GFX9      30
#define HSA_DBG_WATCH_ADDR_MASK_LO_BIT_GFX9_4_3  30
#define HSA_DBG_WATCH_ADDR_MASK_LO_BIT_GFX10     31
#define HSA_DBG_WATCH_ADDR_MASK_HI_BIT           (30 + 32)
#define HSA_DBG_WATCH_ADDR_MASK_HI_BIT_GFX9_4_3  (30 + 32)
#define HSA_DBG_WATCH_ADDR_MASK_HI_BIT_SHIFT     32

#endif /* _LINUXKPI_LINUX_KFD_SYSFS_H */
