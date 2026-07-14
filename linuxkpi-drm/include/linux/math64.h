/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/math64.h> shim (MPL-2.0, original work).
 *
 * 64-bit division/multiply helpers the kernel provides because 32-bit arches lack
 * native 64-bit divide. amdgpu uses them for clock/voltage/frequency math. On
 * x86_64 these are just the native operators — REAL arithmetic, inlined. License
 * boundary (../../README.md): API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_MATH64_H
#define _LINUXKPI_LINUX_MATH64_H

#include <linux/types.h>
#include <asm/div64.h>   /* do_div() — in-place 64/32 division */

static inline u64 div_u64(u64 dividend, u32 divisor) { return dividend / divisor; }
static inline s64 div_s64(s64 dividend, s32 divisor) { return dividend / divisor; }
static inline u64 div64_u64(u64 dividend, u64 divisor) { return dividend / divisor; }
static inline u64 div64_ul(u64 dividend, unsigned long divisor) { return dividend / divisor; }
static inline s64 div64_s64(s64 dividend, s64 divisor) { return dividend / divisor; }
static inline u64 div_u64_rem(u64 dividend, u32 divisor, u32 *rem) { *rem = (u32)(dividend % divisor); return dividend / divisor; }
static inline u64 div64_u64_rem(u64 dividend, u64 divisor, u64 *rem) { *rem = dividend % divisor; return dividend / divisor; }
static inline s64 div_s64_rem(s64 dividend, s32 divisor, s32 *rem) { *rem = (s32)(dividend % divisor); return dividend / divisor; }

static inline u64 mul_u64_u32_shr(u64 a, u32 mul, unsigned int shift)
{ return (u64)(((unsigned __int128)a * mul) >> shift); }
static inline u64 mul_u64_u64_shr(u64 a, u64 b, unsigned int shift)
{ return (u64)(((unsigned __int128)a * b) >> shift); }
static inline u64 mul_u32_u32(u32 a, u32 b) { return (u64)a * b; }

#define DIV_ROUND_UP_ULL(ll, d)      div_u64((ll) + (d) - 1, (d))
#define DIV_ROUND_DOWN_ULL(ll, d)    div_u64((ll), (d))
#define DIV_ROUND_CLOSEST_ULL(x, d)  div_u64((x) + ((d) / 2), (d))

#endif /* _LINUXKPI_LINUX_MATH64_H */
