/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/mmu_notifier.h> shim (MPL-2.0, original work).
 *
 * MMU notifiers — callbacks when the CPU page tables backing a userptr/SVM range
 * change, so the GPU can invalidate its mapping. Not on the MES bring-up path
 * (userptr/SVM is out of subset); reached via amdgpu_hmm.h for type layout. The
 * register/invalidate machinery is backed by ath_linuxkpi at M4 when SVM is in
 * scope. License boundary (../../README.md): API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_MMU_NOTIFIER_H
#define _LINUXKPI_LINUX_MMU_NOTIFIER_H

#include <linux/types.h>

struct mmu_interval_notifier;
struct mm_struct;

enum mmu_notifier_event {
	MMU_NOTIFY_UNMAP,
	MMU_NOTIFY_CLEAR,
	MMU_NOTIFY_PROTECTION_VMA,
	MMU_NOTIFY_PROTECTION_PAGE,
	MMU_NOTIFY_SOFT_DIRTY,
	MMU_NOTIFY_RELEASE,
	MMU_NOTIFY_MIGRATE,
	MMU_NOTIFY_EXCLUSIVE,
};

struct mmu_notifier_range {
	struct mm_struct *mm;
	unsigned long     start;
	unsigned long     end;
	enum mmu_notifier_event event;
	unsigned          flags;
};

struct mmu_interval_notifier_ops {
	bool (*invalidate)(struct mmu_interval_notifier *interval_sub,
			   const struct mmu_notifier_range *range,
			   unsigned long cur_seq);
};

struct mmu_interval_notifier {
	unsigned long start;
	unsigned long last;
	const struct mmu_interval_notifier_ops *ops;
	struct mm_struct *mm;
	unsigned long invalidate_seq;
};

/* backed by ath_linuxkpi (M4) */
int  mmu_interval_notifier_insert(struct mmu_interval_notifier *interval_sub, struct mm_struct *mm,
				  unsigned long start, unsigned long length,
				  const struct mmu_interval_notifier_ops *ops);
void mmu_interval_notifier_remove(struct mmu_interval_notifier *interval_sub);
unsigned long mmu_interval_read_begin(struct mmu_interval_notifier *interval_sub);
bool mmu_interval_read_retry(struct mmu_interval_notifier *interval_sub, unsigned long seq);

static inline void mmu_interval_set_seq(struct mmu_interval_notifier *s, unsigned long seq) { s->invalidate_seq = seq; }

#endif /* _LINUXKPI_LINUX_MMU_NOTIFIER_H */
