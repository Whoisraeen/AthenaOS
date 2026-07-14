/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/kernel.h> shim (MPL-2.0, original work).
 *
 * The kernel "catch-all" hub. CRITICAL: must exist so `#include <linux/kernel.h>`
 * resolves HERE and not the host's /usr/include/linux/kernel.h (which drags in
 * host kernel ABI — it was leaking a host `struct sysinfo` that clashed with
 * mm.h). It re-exports the common helper surface from the focused shims (no own
 * definitions, so nothing double-defines). License boundary: API surface.
 */
#ifndef _LINUXKPI_LINUX_KERNEL_H
#define _LINUXKPI_LINUX_KERNEL_H

#include <linux/types.h>
#include <linux/compiler.h>   /* _THIS_IP_, annotations, DECLARE_FLEX_ARRAY */
#include <linux/math.h>       /* min/max/clamp, ALIGN, DIV_ROUND_UP, roundup, abs, swap */
#include <linux/bitops.h>     /* BIT, GENMASK, fls, ARRAY-bit helpers */
#include <linux/bug.h>        /* BUG/WARN/BUILD_BUG_ON */
#include <linux/printk.h>     /* printk, pr_* */
#include <linux/wordpart.h>   /* upper/lower_32_bits */
#include <linux/limits.h>     /* (via types.h) U*_MAX */
#include <linux/kstrtox.h>    /* kstrtoul/kstrtoint string parse */

#ifndef ARRAY_SIZE
#define ARRAY_SIZE(arr) (sizeof(arr) / sizeof((arr)[0]))
#endif

/* numeric helpers occasionally spelled via kernel.h */
#define DIV_ROUND_UP_SECTOR_T(n, d) DIV_ROUND_UP((n), (d))
#define round_mask(x, y) ((__typeof__(x))((y) - 1))
int  hex_to_bin(unsigned char ch);

#endif /* _LINUXKPI_LINUX_KERNEL_H */
