/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/wordpart.h> shim (MPL-2.0, original work).
 *
 * Extract halves of a word — amdgpu splits 64-bit GPU addresses into the hi/lo
 * register pairs with these constantly. Pure macros. License boundary: surface.
 */
#ifndef _LINUXKPI_LINUX_WORDPART_H
#define _LINUXKPI_LINUX_WORDPART_H

#include <linux/types.h>

#define upper_32_bits(n) ((u32)(((n) >> 16) >> 16))
#define lower_32_bits(n) ((u32)((n) & 0xffffffffU))
#define upper_16_bits(n) ((u16)((n) >> 16))
#define lower_16_bits(n) ((u16)((n) & 0xffffU))
#define REPEAT_BYTE(x)   ((~0UL / 0xff) * (x))

#endif /* _LINUXKPI_LINUX_WORDPART_H */
