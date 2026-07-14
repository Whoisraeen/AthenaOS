/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <asm/div64.h> shim (MPL-2.0, original work).
 *
 * do_div(n, base): in-place 64/32 division — n becomes the quotient, the
 * remainder is the macro's value. On x86_64 plain 64-bit division is a single
 * instruction, so this is the obvious expansion (the kernel's asm/div64.h exists
 * to avoid a libgcc __udivdi3 call on 32-bit; not a concern here). The div_u64
 * family lives in <linux/math64.h>. License boundary (../../README.md): API.
 */
#ifndef _LINUXKPI_ASM_DIV64_H
#define _LINUXKPI_ASM_DIV64_H

#include <linux/types.h>

#ifndef do_div
#define do_div(n, base) ({				\
	u32 __base = (u32)(base);			\
	u32 __rem = (u32)(((u64)(n)) % __base);		\
	(n) = ((u64)(n)) / __base;			\
	__rem;						\
})
#endif

#endif /* _LINUXKPI_ASM_DIV64_H */
