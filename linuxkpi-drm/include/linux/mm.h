/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/mm.h> shim (MPL-2.0, original work).
 *
 * Memory-management API. The MES bring-up subset touches little of this (page
 * allocation + BO mmap are TTM/GEM, stubbed per SCOPE.md), so this provides the
 * pure PAGE_* arithmetic the DRM type graph needs and DECLARES the allocator/map
 * surface — backed by raeen_linuxkpi at M4. A fake alloc_page returning NULL, or
 * a page_address that lied, would corrupt any path that did use it (SCOPE.md
 * rule 9), so those are real-extern, never faked. License boundary: API surface.
 */
#ifndef _LINUXKPI_LINUX_MM_H
#define _LINUXKPI_LINUX_MM_H

#include <linux/types.h>
#include <linux/mm_types.h>
/* the GFP_ and __GFP_ allocation flags + kmalloc live in slab.h; files that
 * include only <linux/mm.h> (e.g. TTM) still spell GFP_KERNEL/__GFP_ZERO. slab.h
 * pulls only types.h, so this is acyclic. */
#include <linux/slab.h>

#ifndef PAGE_SHIFT
#define PAGE_SHIFT 12
#endif
#ifndef PAGE_SIZE
#define PAGE_SIZE  (1UL << PAGE_SHIFT)
#endif
#ifndef PAGE_MASK
#define PAGE_MASK  (~(PAGE_SIZE - 1))
#endif
#define PAGE_ALIGN(addr)     (((addr) + PAGE_SIZE - 1) & PAGE_MASK)
#define PAGE_ALIGNED(addr)   (((unsigned long)(addr) & (PAGE_SIZE - 1)) == 0)
#define offset_in_page(p)    ((unsigned long)(p) & (PAGE_SIZE - 1))
#define PFN_UP(x)            (((x) + PAGE_SIZE - 1) >> PAGE_SHIFT)
#define PFN_DOWN(x)          ((x) >> PAGE_SHIFT)
#define PFN_PHYS(x)          ((phys_addr_t)(x) << PAGE_SHIFT)
#define PHYS_PFN(x)          ((unsigned long)((x) >> PAGE_SHIFT))

/* page <-> address — backed by raeen_linuxkpi (M4) */
unsigned long page_to_pfn(const struct page *page);
struct page  *pfn_to_page(unsigned long pfn);
void         *page_address(const struct page *page);
struct page  *virt_to_page(const void *addr);
#define page_to_virt(page) page_address(page)
phys_addr_t   page_to_phys(const struct page *page);
void          get_page(struct page *page);
void          put_page(struct page *page);

/* page allocation — backed by raeen_linuxkpi (M4) */
struct page  *alloc_pages(gfp_t gfp, unsigned int order);
struct page  *alloc_page(gfp_t gfp);
unsigned long __get_free_pages(gfp_t gfp, unsigned int order);
void          __free_pages(struct page *page, unsigned int order);
void          free_pages(unsigned long addr, unsigned int order);

/* vmalloc family — backed by raeen_linuxkpi (M4) */
void *vmalloc(unsigned long size);
void *vzalloc(unsigned long size);
void  vfree(const void *addr);
void *kvmalloc(size_t size, gfp_t gfp);
void *kvmalloc_array(size_t n, size_t size, gfp_t gfp);
void *kvzalloc(size_t size, gfp_t gfp);
void *kvcalloc(size_t n, size_t size, gfp_t gfp);
void  kvfree(const void *addr);
bool  is_vmalloc_addr(const void *x);

/* system memory info (si_meminfo) — amdgpu sizes GTT from total RAM. Backed M4.
 * Independently guarded: <linux/mm.h> can be reached twice through the post-fs.h
 * include graph, and only this struct (not the macro-guarded rest) double-defined. */
#ifndef _LINUXKPI_STRUCT_SYSINFO
#define _LINUXKPI_STRUCT_SYSINFO
struct sysinfo {
	long   uptime;
	unsigned long totalram;
	unsigned long freeram;
	unsigned long sharedram;
	unsigned long bufferram;
	unsigned long totalswap;
	unsigned long freeswap;
	unsigned long totalhigh;
	unsigned long freehigh;
	unsigned int  mem_unit;
};
#endif /* _LINUXKPI_STRUCT_SYSINFO */
void si_meminfo(struct sysinfo *val);

/* vma->vm_flags bits (the subset amdgpu's mmap/fault paths inspect). */
#define VM_NONE       0x00000000
#define VM_READ       0x00000001
#define VM_WRITE      0x00000002
#define VM_EXEC       0x00000004
#define VM_SHARED     0x00000008
#define VM_MAYREAD    0x00000010
#define VM_MAYWRITE   0x00000020
#define VM_MAYEXEC    0x00000040
#define VM_MAYSHARE   0x00000080
#define VM_PFNMAP     0x00000400
#define VM_IO         0x00004000
#define VM_DONTEXPAND 0x00040000
#define VM_DONTDUMP   0x04000000
#define VM_MIXEDMAP   0x10000000
#define VM_ACCESS_FLAGS (VM_READ | VM_WRITE | VM_EXEC)

/* Linux 7's shmem setup interface takes an opaque vma_flags_t bitmap rather
 * than the legacy unsigned-long mask.  The numeric bit positions are part of
 * the in-kernel API; keep the reachable subset identical to upstream. */
typedef int vma_flag_t;
enum {
	VMA_READ_BIT       = 0,
	VMA_WRITE_BIT      = 1,
	VMA_EXEC_BIT       = 2,
	VMA_SHARED_BIT     = 3,
	VMA_MAYREAD_BIT    = 4,
	VMA_MAYWRITE_BIT   = 5,
	VMA_MAYEXEC_BIT    = 6,
	VMA_MAYSHARE_BIT   = 7,
	VMA_PFNMAP_BIT     = 10,
	VMA_IO_BIT         = 14,
	VMA_DONTEXPAND_BIT = 18,
	VMA_NORESERVE_BIT  = 21,
	VMA_DONTDUMP_BIT   = 26,
	VMA_MIXEDMAP_BIT   = 28,
};

static inline vma_flags_t __mk_vma_flags(size_t count,
					 const vma_flag_t *bits)
{
	vma_flags_t flags = EMPTY_VMA_FLAGS;
	size_t i;

	for (i = 0; i < count; i++) {
		if (bits[i] >= 0 && (unsigned int)bits[i] < 8U * sizeof(unsigned long))
			flags.__vma_flags[0] |= 1UL << bits[i];
	}
	return flags;
}

#define mk_vma_flags(...) \
	__mk_vma_flags(sizeof((const vma_flag_t[]){ __VA_ARGS__ }) / \
			 sizeof(vma_flag_t), \
			 (const vma_flag_t[]){ __VA_ARGS__ })

/* vmf->flags bits (page-fault context flags). */
#define FAULT_FLAG_WRITE        0x01
#define FAULT_FLAG_MKWRITE      0x02
#define FAULT_FLAG_ALLOW_RETRY  0x04
#define FAULT_FLAG_RETRY_NOWAIT 0x08
#define FAULT_FLAG_KILLABLE     0x10
#define FAULT_FLAG_USER         0x40

/* vm_fault_t return codes (page-fault handler results). amdgpu's BO fault handler
 * returns these. Values match the kernel ABI. */
#define VM_FAULT_NOPAGE   0x000100
#define VM_FAULT_SIGBUS   0x000002
#define VM_FAULT_OOM      0x000001
#define VM_FAULT_HWPOISON 0x000010
#define VM_FAULT_HWPOISON_LARGE 0x000020
#define VM_FAULT_SIGSEGV  0x000040
#define VM_FAULT_RETRY    0x000400
#define VM_FAULT_FALLBACK 0x000800
#define VM_FAULT_ERROR (VM_FAULT_OOM | VM_FAULT_SIGBUS | VM_FAULT_SIGSEGV | \
			VM_FAULT_HWPOISON | VM_FAULT_HWPOISON_LARGE | VM_FAULT_FALLBACK)

/* page-order of an allocation size (smallest order whose pages cover `size`). */
static inline int get_order(unsigned long size)
{
	int order = 0;
	if (size == 0)
		return 0;
	size = (size - 1) >> PAGE_SHIFT;
	while (size) { order++; size >>= 1; }
	return order;
}
/* a copy-on-write private mapping: writable but not shared. */
static inline bool is_cow_mapping(unsigned long flags)
{
	return (flags & (VM_SHARED | VM_MAYWRITE)) == VM_MAYWRITE;
}
static inline void vm_flags_clear(struct vm_area_struct *vma, unsigned long flags)
{
	vma->vm_flags &= ~flags;
}
static inline void vm_flags_set(struct vm_area_struct *vma, unsigned long flags)
{
	vma->vm_flags |= flags;
}

/* AthenaOS records cache mode in pgprot_t; access rights remain enforced by the
 * VM mapping syscall.  Start from the normal WB encoding, after which callers
 * such as drm_gem_mmap_obj() can select WC explicitly. */
static inline pgprot_t vm_get_page_prot(vm_flags_t flags)
{
	(void)flags;
	return (pgprot_t){ 0UL };
}

/* mmap insertion — backed by raeen_linuxkpi (M4) */
int        remap_pfn_range(struct vm_area_struct *vma, unsigned long addr, unsigned long pfn,
			   unsigned long size, pgprot_t prot);
int        vm_insert_page(struct vm_area_struct *vma, unsigned long addr, struct page *page);
vm_fault_t vmf_insert_pfn(struct vm_area_struct *vma, unsigned long addr, unsigned long pfn);

#endif /* _LINUXKPI_LINUX_MM_H */
