/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/math.h> shim (MPL-2.0, original work).
 *
 * min/max/clamp, rounding, and alignment macros — used on nearly every line of
 * amdgpu (register field rounding, ring-size alignment, BO size math). Pure
 * macros, the obvious arithmetic the names dictate. License boundary
 * (../../README.md): API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_MATH_H
#define _LINUXKPI_LINUX_MATH_H

#include <linux/types.h>

#define min(a, b) ({ __typeof__(a) _amin = (a); __typeof__(b) _bmin = (b); _amin < _bmin ? _amin : _bmin; })
#define max(a, b) ({ __typeof__(a) _amax = (a); __typeof__(b) _bmax = (b); _amax > _bmax ? _amax : _bmax; })
#define min_t(type, a, b) ({ type _amint = (type)(a); type _bmint = (type)(b); _amint < _bmint ? _amint : _bmint; })
#define max_t(type, a, b) ({ type _amaxt = (type)(a); type _bmaxt = (type)(b); _amaxt > _bmaxt ? _amaxt : _bmaxt; })
#define min3(a, b, c) min(min(a, b), c)
#define max3(a, b, c) max(max(a, b), c)
/* the smaller of two values, treating 0 as "no limit" (returns the other). */
#define min_not_zero(x, y) ({ __typeof__(x) __x = (x); __typeof__(y) __y = (y); \
	__x == 0 ? __y : (__y == 0 ? __x : min(__x, __y)); })
/* Upper-case MAX/MIN are the constant-expression forms (usable in array sizes,
 * e.g. smu_cmn's sort_feature[MAX(...)]) — no statement-expression. */
#ifndef MAX
#define MAX(a, b) ((a) > (b) ? (a) : (b))
#endif
#ifndef MIN
#define MIN(a, b) ((a) < (b) ? (a) : (b))
#endif
#define clamp(val, lo, hi)            max((lo), min((val), (hi)))
#define clamp_t(type, val, lo, hi)    max_t(type, (lo), min_t(type, (val), (hi)))
#define clamp_val(val, lo, hi)        clamp_t(__typeof__(val), val, lo, hi)

#define abs(x) ({ __typeof__(x) _ax = (x); _ax < 0 ? -_ax : _ax; })
#define swap(a, b) do { __typeof__(a) _swt = (a); (a) = (b); (b) = _swt; } while (0)

#define DIV_ROUND_UP(n, d)      (((n) + (d) - 1) / (d))
#define DIV_ROUND_DOWN(n, d)    ((n) / (d))
#define DIV_ROUND_CLOSEST(x, d) (((x) + ((d) / 2)) / (d))
#define roundup(x, y)           (DIV_ROUND_UP((x), (y)) * (y))
#define rounddown(x, y)         (((x) / (y)) * (y))
#define mult_frac(x, n, d)      (((x) / (d)) * (n) + ((((x) % (d)) * (n)) / (d)))

/* power-of-two alignment */
#define __round_mask(x, y)      ((__typeof__(x))((y) - 1))
#define round_up(x, y)          ((((x) - 1) | __round_mask(x, y)) + 1)
#define round_down(x, y)        ((x) & ~__round_mask(x, y))
#define ALIGN(x, a)             round_up((x), (a))
#define ALIGN_DOWN(x, a)        round_down((x), (a))
#define PTR_ALIGN(p, a)         ((__typeof__(p))ALIGN((unsigned long)(p), (a)))
#define IS_ALIGNED(x, a)        (((x) & ((__typeof__(x))(a) - 1)) == 0)

#endif /* _LINUXKPI_LINUX_MATH_H */
