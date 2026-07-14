/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/pgtable.h> shim (MPL-2.0, original work).
 *
 * Page-protection / cache-attribute helpers. TTM tags BO mappings write-combine
 * vs uncached via pgprot_t. The cache mode genuinely matters for GPU correctness
 * (a framebuffer wants WC), but it is ENFORCED at map time by raeen_linuxkpi's
 * ioremap_wc/ioremap_uc (M5), not by the pgprot value carried around in TTM
 * bookkeeping — so here the modifiers record the requested mode in spare bits and
 * pass through. This is the bookkeeping surface, not a claim that caching is
 * applied here. License boundary (../../README.md): API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_PGTABLE_H
#define _LINUXKPI_LINUX_PGTABLE_H

#include <linux/types.h>
#include <linux/mm_types.h>   /* pgprot_t */
#include <asm/processor.h>    /* boot_cpu_data — arch CPU info, reached via pgtable */

#define pgprot_val(x)   ((x).pgprot)
#define __pgprot(x)     ((pgprot_t){ (x) })

/* requested-cache-mode tag bits (applied for real at ioremap time, M5). */
#define _PAGE_CACHE_WB  0x0UL
#define _PAGE_CACHE_WC  0x1UL
#define _PAGE_CACHE_UC  0x2UL

#define PAGE_KERNEL          __pgprot(_PAGE_CACHE_WB)
#define PAGE_KERNEL_IO       __pgprot(_PAGE_CACHE_UC)
#define PAGE_KERNEL_NOCACHE  __pgprot(_PAGE_CACHE_UC)

static inline pgprot_t pgprot_writecombine(pgprot_t prot) { return __pgprot((pgprot_val(prot) & ~0x3UL) | _PAGE_CACHE_WC); }
static inline pgprot_t pgprot_noncached(pgprot_t prot)    { return __pgprot((pgprot_val(prot) & ~0x3UL) | _PAGE_CACHE_UC); }
static inline pgprot_t pgprot_device(pgprot_t prot)       { return pgprot_noncached(prot); }
static inline pgprot_t pgprot_decrypted(pgprot_t prot)    { return prot; }

#endif /* _LINUXKPI_LINUX_PGTABLE_H */
