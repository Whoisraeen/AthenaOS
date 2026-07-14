/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/hmm.h> shim (MPL-2.0, original work).
 *
 * Heterogeneous Memory Management — CPU<->GPU shared address space (userptr/SVM).
 * Not on the MES bring-up path; reached via amdgpu_hmm.h for type layout. The
 * range-fault machinery is backed by ath_linuxkpi at M4 when SVM is brought into
 * scope; here it is the type + decl surface (the pfn flag bits are kernel ABI).
 * License boundary (../../README.md): API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_HMM_H
#define _LINUXKPI_LINUX_HMM_H

#include <linux/types.h>

#define HMM_PFN_VALID   (1UL << 63)
#define HMM_PFN_WRITE   (1UL << 62)
#define HMM_PFN_ERROR   (1UL << 61)
#define HMM_PFN_FLAGS   (0xfUL << 60)
#define HMM_PFN_REQ_FAULT (1UL << 0)
#define HMM_PFN_REQ_WRITE (1UL << 1)

struct mmu_interval_notifier;

struct hmm_range {
	struct mmu_interval_notifier *notifier;
	unsigned long  notifier_seq;
	unsigned long  start;
	unsigned long  end;
	unsigned long *hmm_pfns;
	unsigned long  default_flags;
	unsigned long  pfn_flags_mask;
	void          *dev_private_owner;
};

int hmm_range_fault(struct hmm_range *range);

#endif /* _LINUXKPI_LINUX_HMM_H */
