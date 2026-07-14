/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/nospec.h> shim (MPL-2.0, original work).
 *
 * Spectre-v1 array-index sanitisation. amdgpu clamps user-supplied indices with
 * array_index_nospec() before using them. The semantic (return the index if in
 * bounds, else 0) is preserved here as a real bounds-clamp; the speculation-
 * barrier nuance is a hardening detail, not a correctness fake. License boundary
 * (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_NOSPEC_H
#define _LINUXKPI_LINUX_NOSPEC_H

#include <linux/types.h>

#define array_index_nospec(index, size) \
	({ __typeof__(index) __i = (index); __typeof__(size) __s = (size); \
	   (__i < __s) ? __i : 0; })

#endif /* _LINUXKPI_LINUX_NOSPEC_H */
