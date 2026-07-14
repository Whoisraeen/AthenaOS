/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/bitmap.h> shim (MPL-2.0, original work).
 *
 * Multi-word bitmap ops over <linux/bitops.h>. amdgpu/KFD track CU masks, queue
 * allocation, and IP-block presence as bitmaps. REAL inline implementations
 * (memset/per-word loops/popcount — the algorithms the names dictate), no fakes.
 * License boundary (../../README.md): API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_BITMAP_H
#define _LINUXKPI_LINUX_BITMAP_H

#include <linux/types.h>
#include <linux/bitops.h>

static inline void bitmap_zero(unsigned long *dst, unsigned int nbits)
{ __builtin_memset(dst, 0, BITS_TO_LONGS(nbits) * sizeof(unsigned long)); }
static inline void bitmap_fill(unsigned long *dst, unsigned int nbits)
{ __builtin_memset(dst, 0xff, BITS_TO_LONGS(nbits) * sizeof(unsigned long)); }
static inline void bitmap_copy(unsigned long *dst, const unsigned long *src, unsigned int nbits)
{ __builtin_memcpy(dst, src, BITS_TO_LONGS(nbits) * sizeof(unsigned long)); }

static inline unsigned int bitmap_weight(const unsigned long *src, unsigned int nbits)
{
	unsigned int k, w = 0, lim = BITS_TO_LONGS(nbits);
	for (k = 0; k < lim; k++)
		w += hweight_long(src[k]);
	return w;
}
static inline bool bitmap_empty(const unsigned long *src, unsigned int nbits)
{
	unsigned int k, lim = nbits / BITS_PER_LONG;
	for (k = 0; k < lim; k++)
		if (src[k]) return false;
	if (nbits % BITS_PER_LONG)
		if (src[k] & (BIT_MASK(nbits) - 1)) return false;
	return true;
}
static inline bool bitmap_full(const unsigned long *src, unsigned int nbits)
{
	unsigned int k, lim = nbits / BITS_PER_LONG;
	for (k = 0; k < lim; k++)
		if (~src[k]) return false;
	return true;
}

static inline void bitmap_set(unsigned long *map, unsigned int start, unsigned int len)
{ unsigned int i; for (i = start; i < start + len; i++) __set_bit(i, map); }
static inline void bitmap_clear(unsigned long *map, unsigned int start, unsigned int len)
{ unsigned int i; for (i = start; i < start + len; i++) __clear_bit(i, map); }

static inline void bitmap_and(unsigned long *dst, const unsigned long *a, const unsigned long *b, unsigned int nbits)
{ unsigned int k, lim = BITS_TO_LONGS(nbits); for (k = 0; k < lim; k++) dst[k] = a[k] & b[k]; }
static inline void bitmap_or(unsigned long *dst, const unsigned long *a, const unsigned long *b, unsigned int nbits)
{ unsigned int k, lim = BITS_TO_LONGS(nbits); for (k = 0; k < lim; k++) dst[k] = a[k] | b[k]; }
static inline void bitmap_xor(unsigned long *dst, const unsigned long *a, const unsigned long *b, unsigned int nbits)
{ unsigned int k, lim = BITS_TO_LONGS(nbits); for (k = 0; k < lim; k++) dst[k] = a[k] ^ b[k]; }
static inline void bitmap_andnot(unsigned long *dst, const unsigned long *a, const unsigned long *b, unsigned int nbits)
{ unsigned int k, lim = BITS_TO_LONGS(nbits); for (k = 0; k < lim; k++) dst[k] = a[k] & ~b[k]; }
static inline void bitmap_complement(unsigned long *dst, const unsigned long *src, unsigned int nbits)
{ unsigned int k, lim = BITS_TO_LONGS(nbits); for (k = 0; k < lim; k++) dst[k] = ~src[k]; }

static inline bool bitmap_equal(const unsigned long *a, const unsigned long *b, unsigned int nbits)
{ unsigned int k, lim = nbits / BITS_PER_LONG; for (k = 0; k < lim; k++) if (a[k] != b[k]) return false; return true; }

#endif /* _LINUXKPI_LINUX_BITMAP_H */
