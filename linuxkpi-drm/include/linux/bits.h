/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/bits.h> shim (MPL-2.0, original work).
 *
 * The low-level bit-constant macros (BIT/GENMASK/BITS_PER_*) that bitops.h builds
 * on. Guarded so it coexists with the identical definitions in <linux/bitops.h>.
 * Pure macros. License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_BITS_H
#define _LINUXKPI_LINUX_BITS_H

#include <linux/types.h>

#ifndef BITS_PER_BYTE
#define BITS_PER_BYTE       8
#endif
#ifndef BITS_PER_LONG
#define BITS_PER_LONG       64
#endif
#ifndef BITS_PER_LONG_LONG
#define BITS_PER_LONG_LONG  64
#endif
#ifndef BIT
#define BIT(nr)        (1UL << (nr))
#endif
#ifndef BIT_ULL
#define BIT_ULL(nr)    (1ULL << (nr))
#endif
#ifndef GENMASK
#define GENMASK(h, l)     (((~0UL) << (l)) & (~0UL >> (BITS_PER_LONG - 1 - (h))))
#endif
#ifndef GENMASK_ULL
#define GENMASK_ULL(h, l) (((~0ULL) << (l)) & (~0ULL >> (BITS_PER_LONG_LONG - 1 - (h))))
#endif

#endif /* _LINUXKPI_LINUX_BITS_H */
