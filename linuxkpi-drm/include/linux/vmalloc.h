/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/vmalloc.h> shim (MPL-2.0, original work).
 *
 * Virtually-contiguous allocations + page-array mapping. amdgpu uses vmalloc for
 * large firmware/IP-discovery buffers and vmap to make a BO's pages CPU-visible.
 * Backed by ath_linuxkpi at M4 (declaration-only — a fake vmap returning NULL
 * would break firmware load; SCOPE.md rule 9). Signatures match <linux/mm.h>'s
 * (identical redeclarations are legal C). License boundary: API surface.
 */
#ifndef _LINUXKPI_LINUX_VMALLOC_H
#define _LINUXKPI_LINUX_VMALLOC_H

#include <linux/types.h>
#include <linux/mm_types.h>   /* struct page, pgprot_t */

void *vmalloc(unsigned long size);
void *vzalloc(unsigned long size);
void *vmalloc_user(unsigned long size);
void  vfree(const void *addr);
void *kvmalloc(size_t size, gfp_t gfp);
void *kvzalloc(size_t size, gfp_t gfp);
void *kvcalloc(size_t n, size_t size, gfp_t gfp);
void  kvfree(const void *addr);
bool  is_vmalloc_addr(const void *x);

/* map an array of pages into a contiguous kernel-visible range — backed M4. */
void *vmap(struct page **pages, unsigned int count, unsigned long flags, pgprot_t prot);
void  vunmap(const void *addr);

#define VM_MAP      0x00000004
#define VM_IOREMAP  0x00000001

#endif /* _LINUXKPI_LINUX_VMALLOC_H */
