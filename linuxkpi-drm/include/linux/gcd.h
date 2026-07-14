/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/gcd.h> shim (MPL-2.0, original work).
 *
 * Greatest common divisor — amdgpu_pll uses it to reduce feedback/reference
 * divider ratios when computing display/engine PLL settings. Backed by
 * raeen_linuxkpi's lib/gcd binary-GCD implementation at M4. License boundary
 * (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_GCD_H
#define _LINUXKPI_LINUX_GCD_H

#include <linux/types.h>

unsigned long gcd(unsigned long a, unsigned long b);

#endif /* _LINUXKPI_LINUX_GCD_H */
