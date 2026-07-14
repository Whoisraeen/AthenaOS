/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/overflow.h> shim (MPL-2.0, original work).
 *
 * Overflow-checked size arithmetic. amdgpu sizes many allocations with
 * struct_size()/array_size() so a multiply can't wrap to a small allocation
 * (a classic heap-overflow vector). Built on the compiler's __builtin_*_overflow;
 * on overflow the size saturates to SIZE_MAX so the subsequent kmalloc fails
 * rather than under-allocating. License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_OVERFLOW_H
#define _LINUXKPI_LINUX_OVERFLOW_H

#include <linux/types.h>

#define check_add_overflow(a, b, d) __builtin_add_overflow(a, b, d)
#define check_sub_overflow(a, b, d) __builtin_sub_overflow(a, b, d)
#define check_mul_overflow(a, b, d) __builtin_mul_overflow(a, b, d)

static inline size_t size_mul(size_t a, size_t b)
{
	size_t r;
	if (__builtin_mul_overflow(a, b, &r))
		return SIZE_MAX;
	return r;
}
static inline size_t size_add(size_t a, size_t b)
{
	size_t r;
	if (__builtin_add_overflow(a, b, &r))
		return SIZE_MAX;
	return r;
}
static inline size_t size_sub(size_t a, size_t b)
{
	size_t r;
	if (a < b || __builtin_sub_overflow(a, b, &r))
		return SIZE_MAX;
	return r;
}

#define array_size(a, b)        size_mul(a, b)
#define array3_size(a, b, c)    size_mul(size_mul(a, b), c)
#define flex_array_size(p, member, count) \
	size_mul(count, sizeof(*(p)->member) + (size_t)0)
#define struct_size(p, member, count) \
	size_add(sizeof(*(p)), size_mul(count, sizeof(*(p)->member)))
#define struct_size_t(type, member, count) \
	size_add(sizeof(type), size_mul(count, sizeof(((type *)0)->member[0])))

/* Range bounds checks used by the drm-core allocators (drm_buddy / drm_mm) to
 * reject an allocation/trim that would fall outside [0, max). Pure arithmetic
 * expressions of the documented kernel contract. */
#define range_overflows(start, size, max) ({ \
	typeof(start) start__ = (start); \
	typeof(size) size__ = (size); \
	typeof(max) max__ = (max); \
	(void)(&start__ == &size__); \
	(void)(&start__ == &max__); \
	start__ >= max__ || size__ > max__ - start__; \
})
#define range_overflows_t(type, start, size, max) \
	range_overflows((type)(start), (type)(size), (type)(max))
#define range_end_overflows(start, size, max) ({ \
	typeof(start) start__ = (start); \
	typeof(size) size__ = (size); \
	typeof(max) max__ = (max); \
	(void)(&start__ == &size__); \
	(void)(&start__ == &max__); \
	size__ > max__ - start__; \
})

#endif /* _LINUXKPI_LINUX_OVERFLOW_H */
