/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/mm_types.h> shim (MPL-2.0, original work).
 *
 * Core memory-management types. The MES bring-up subset does not touch the page
 * allocator or BO mmap fault path (that is TTM/GEM territory, stubbed per
 * SCOPE.md), so these are the minimal definitions the DRM type graph needs for
 * layout — `struct page` is treated opaquely (accessed via page_to_pfn/
 * page_address helpers elsewhere), and `vm_area_struct`/`vm_operations_struct`
 * carry the fields a GEM mmap handler reads, available for when that path is
 * actually compiled. License boundary (../../README.md): API surface only.
 */
#ifndef _LINUXKPI_LINUX_MM_TYPES_H
#define _LINUXKPI_LINUX_MM_TYPES_H

#include <linux/types.h>
#include <linux/atomic.h>

struct mm_struct;
struct address_space;

/* Opaque to the MES path; real layout lives in the M5 page facade if TTM lands.
 * TTM sets page->mapping/->index on the BO's backing pages, so those members are
 * present for layout (the page facade owns their real meaning at M5). */
struct page {
	unsigned long          flags;
	atomic_t               _refcount;
	void                  *virtual_addr;
	struct address_space  *mapping;
	unsigned long          index;
	void                  *private_data;
	struct list_head       lru;      /* TTM pool chains free pages here */
	unsigned long          private;  /* TTM stashes the page order here */
	/* AthenaOS LinuxKPI backing identity. These are private facade fields,
	 * never consumed by upstream code: the daemon VA and DMA/physical address
	 * are intentionally distinct, and the allocation token releases the host
	 * IOMMU mapping on the head page. */
	u64                    rae_dma_addr;
	u64                    rae_dma_token;
};
struct file;

typedef struct { unsigned long pgprot; } pgprot_t;
typedef unsigned long vm_flags_t;
typedef int vm_fault_t;

/* Linux 7 represents the mutable VMA flags passed to shmem as an opaque
 * one-word bitmap.  Keep that by-value ABI exact even though AthenaOS does not
 * yet provide shmem-backed GEM objects. */
typedef struct {
	unsigned long __vma_flags[1];
} vma_flags_t;

#define EMPTY_VMA_FLAGS ((vma_flags_t){ { 0UL } })

struct vm_area_struct {
	unsigned long vm_start;
	unsigned long vm_end;
	vm_flags_t    vm_flags;
	unsigned long vm_pgoff;
	pgprot_t      vm_page_prot;
	void         *vm_private_data;
	const struct vm_operations_struct *vm_ops;
	struct mm_struct *vm_mm;
	struct file  *vm_file;
};

struct vm_fault {
	struct vm_area_struct *vma;
	unsigned int  flags;          /* FAULT_FLAG_* */
	unsigned long address;
	pgoff_t pgoff;
	struct page *page;
};

struct vm_operations_struct {
	void (*open)(struct vm_area_struct *area);
	void (*close)(struct vm_area_struct *area);
	vm_fault_t (*fault)(struct vm_fault *vmf);
	int  (*access)(struct vm_area_struct *vma, unsigned long addr, void *buf, int len, int write);
};

#endif /* _LINUXKPI_LINUX_MM_TYPES_H */
