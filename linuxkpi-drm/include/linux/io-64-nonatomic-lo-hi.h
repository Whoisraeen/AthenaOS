/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/io-64-nonatomic-lo-hi.h> shim (MPL-2.0, original work).
 *
 * 64-bit MMIO as two 32-bit accesses, low word first. amdgpu uses this for GPU
 * registers that are a 64-bit value split across two 32-bit register slots (GART
 * base, fence addresses) where a true 64-bit bus access isn't guaranteed. REAL —
 * built on the real readl/writel (io.h). License boundary: API surface.
 */
#ifndef _LINUXKPI_LINUX_IO_64_NONATOMIC_LO_HI_H
#define _LINUXKPI_LINUX_IO_64_NONATOMIC_LO_HI_H

#include <linux/io.h>

static inline u64 lo_hi_readq(const volatile void __iomem *addr)
{
	const volatile u32 __iomem *p = (const volatile u32 __iomem *)addr;
	u32 lo = readl(p);
	u32 hi = readl((const volatile void __iomem *)((const char *)p + 4));
	return lo | ((u64)hi << 32);
}
static inline void lo_hi_writeq(u64 val, volatile void __iomem *addr)
{
	volatile u32 __iomem *p = (volatile u32 __iomem *)addr;
	writel((u32)val, p);
	writel((u32)(val >> 32), (volatile void __iomem *)((char *)p + 4));
}

#ifndef readq
#define readq  lo_hi_readq
#endif
#ifndef writeq
#define writeq lo_hi_writeq
#endif

#endif /* _LINUXKPI_LINUX_IO_64_NONATOMIC_LO_HI_H */
