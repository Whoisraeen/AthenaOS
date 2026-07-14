/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/io.h> shim (MPL-2.0, original work).
 *
 * MMIO + ioremap. This is THE register-access path: amdgpu's RREG32/WREG32 (and
 * the SOC15 macros) bottom out in readl/writel against the ioremap'd BAR. These
 * MUST be real at runtime — a faked readl/writel would mean we never touch the
 * GPU (the whole point), so they are declaration-only, backed by ath_linuxkpi's
 * ioremap + MMIO facade (P2) at M4/M5. NO inline fakes (SCOPE.md rule 9). License
 * boundary (../../README.md): API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_IO_H
#define _LINUXKPI_LINUX_IO_H

#include <linux/types.h>
#include <linux/memremap.h>   /* memremap() + MEMREMAP_* — <linux/io.h> exposes these */

/* register read/write — backed by ath_linuxkpi's MMIO facade (M4/M5) */
u8  readb(const volatile void __iomem *addr);
u16 readw(const volatile void __iomem *addr);
u32 readl(const volatile void __iomem *addr);
u64 readq(const volatile void __iomem *addr);
void writeb(u8  val, volatile void __iomem *addr);
void writew(u16 val, volatile void __iomem *addr);
void writel(u32 val, volatile void __iomem *addr);
void writeq(u64 val, volatile void __iomem *addr);

/* relaxed variants (no ordering barrier) — same backing on x86 */
#define readl_relaxed(a)      readl(a)
#define writel_relaxed(v, a)  writel((v), (a))
#define readq_relaxed(a)      readq(a)
#define writeq_relaxed(v, a)  writeq((v), (a))

/* aperture mapping — backed by ath_linuxkpi (M4) */
void __iomem *ioremap(phys_addr_t offset, size_t size);
void __iomem *ioremap_wc(phys_addr_t offset, size_t size);
void __iomem *ioremap_cache(phys_addr_t offset, size_t size);
void iounmap(volatile void __iomem *addr);

void memset_io(volatile void __iomem *dst, int c, size_t count);
void memcpy_toio(volatile void __iomem *dst, const void *src, size_t count);
void memcpy_fromio(void *dst, const volatile void __iomem *src, size_t count);

#endif /* _LINUXKPI_LINUX_IO_H */
