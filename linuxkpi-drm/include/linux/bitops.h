/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/bitops.h> shim (MPL-2.0, original work).
 *
 * Bit/bitmap vocabulary. amdgpu leans on this everywhere (register field masks
 * via BIT/GENMASK, per-ring/per-queue bitmaps via set_bit/test_bit, IP-block
 * iteration via for_each_set_bit). All REAL — atomic bit ops over the GCC atomic
 * builtins, the rest pure arithmetic/popcount/clz the names dictate. No backing,
 * no fakes. License boundary (../../README.md): API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_BITOPS_H
#define _LINUXKPI_LINUX_BITOPS_H

#include <linux/types.h>
#include <linux/atomic.h>

#define BITS_PER_BYTE       8
#define BITS_PER_LONG       64
#define BITS_PER_LONG_LONG  64

#define BIT(nr)        (1UL << (nr))
#define BIT_ULL(nr)    (1ULL << (nr))
#define BIT_MASK(nr)   (1UL << ((nr) % BITS_PER_LONG))
#define BIT_WORD(nr)   ((nr) / BITS_PER_LONG)
#define BITS_PER_TYPE(t) (sizeof(t) * BITS_PER_BYTE)
#define BITS_TO_LONGS(nr) (((nr) + BITS_PER_LONG - 1) / BITS_PER_LONG)
#define DECLARE_BITMAP(name, bits) unsigned long name[BITS_TO_LONGS(bits)]

#define GENMASK(h, l) \
	(((~0UL) << (l)) & (~0UL >> (BITS_PER_LONG - 1 - (h))))
#define GENMASK_ULL(h, l) \
	(((~0ULL) << (l)) & (~0ULL >> (BITS_PER_LONG_LONG - 1 - (h))))

/* ---- scan/count (pure, compiler builtins) ---- */
static inline int   fls(unsigned int x)        { return x ? 32 - __builtin_clz(x) : 0; }
static inline int   fls64(u64 x)               { return x ? 64 - __builtin_clzll(x) : 0; }
static inline int   __fls(unsigned long x)     { return 63 - __builtin_clzll(x); }
static inline unsigned long __ffs(unsigned long x) { return __builtin_ctzll(x); }
static inline int   ffs(int x)                 { return x ? __builtin_ctz((unsigned)x) + 1 : 0; }
static inline unsigned int hweight8(u8 w)      { return __builtin_popcount(w); }
static inline unsigned int hweight16(u16 w)    { return __builtin_popcount(w); }
static inline unsigned int hweight32(u32 w)    { return __builtin_popcount(w); }
static inline unsigned int hweight64(u64 w)    { return __builtin_popcountll(w); }
#define hweight_long(w) hweight64((u64)(w))

#ifndef ilog2
#define ilog2(n) ((unsigned)(8 * sizeof(unsigned long long) - __builtin_clzll((unsigned long long)(n)) - 1))
#endif
static inline bool is_power_of_2(unsigned long n) { return n != 0 && ((n & (n - 1)) == 0); }
static inline unsigned long roundup_pow_of_two(unsigned long n) { return n <= 1 ? 1 : 1UL << fls64(n - 1); }
static inline unsigned long rounddown_pow_of_two(unsigned long n) { return n ? 1UL << __fls(n) : 0; }
#define order_base_2(n) ((n) <= 1 ? 0 : ilog2((n) - 1) + 1)

/* ---- atomic bit ops on a bitmap word array ---- */
static inline void set_bit(unsigned int nr, volatile unsigned long *addr)
{ __atomic_or_fetch(&((unsigned long *)addr)[BIT_WORD(nr)], BIT_MASK(nr), __ATOMIC_SEQ_CST); }
static inline void clear_bit(unsigned int nr, volatile unsigned long *addr)
{ __atomic_and_fetch(&((unsigned long *)addr)[BIT_WORD(nr)], ~BIT_MASK(nr), __ATOMIC_SEQ_CST); }
/* clear_bit with release ordering (drm_mm uses it to publish a freed node's
 * scanned state); the SEQ_CST clear_bit above already carries the barrier. */
static inline void clear_bit_unlock(unsigned int nr, volatile unsigned long *addr)
{ clear_bit(nr, addr); }
static inline void change_bit(unsigned int nr, volatile unsigned long *addr)
{ __atomic_xor_fetch(&((unsigned long *)addr)[BIT_WORD(nr)], BIT_MASK(nr), __ATOMIC_SEQ_CST); }
static inline int  test_bit(unsigned int nr, const volatile unsigned long *addr)
{ return (__atomic_load_n(&((const unsigned long *)addr)[BIT_WORD(nr)], __ATOMIC_RELAXED) >> ((nr) % BITS_PER_LONG)) & 1UL; }
static inline int  test_and_set_bit(unsigned int nr, volatile unsigned long *addr)
{ unsigned long old = __atomic_fetch_or(&((unsigned long *)addr)[BIT_WORD(nr)], BIT_MASK(nr), __ATOMIC_SEQ_CST); return (old & BIT_MASK(nr)) != 0; }
static inline int  test_and_clear_bit(unsigned int nr, volatile unsigned long *addr)
{ unsigned long old = __atomic_fetch_and(&((unsigned long *)addr)[BIT_WORD(nr)], ~BIT_MASK(nr), __ATOMIC_SEQ_CST); return (old & BIT_MASK(nr)) != 0; }

/* non-atomic variants (single-thread fast paths) */
static inline void __set_bit(unsigned int nr, volatile unsigned long *addr)   { ((unsigned long *)addr)[BIT_WORD(nr)] |= BIT_MASK(nr); }
static inline void __clear_bit(unsigned int nr, volatile unsigned long *addr) { ((unsigned long *)addr)[BIT_WORD(nr)] &= ~BIT_MASK(nr); }

/* ---- bitmap scan (real loops) ---- */
unsigned long find_first_bit(const unsigned long *addr, unsigned long size);
unsigned long find_next_bit(const unsigned long *addr, unsigned long size, unsigned long offset);
unsigned long find_first_zero_bit(const unsigned long *addr, unsigned long size);
unsigned long find_next_zero_bit(const unsigned long *addr, unsigned long size, unsigned long offset);

#define for_each_set_bit(bit, addr, size) \
	for ((bit) = find_first_bit((addr), (size)); (bit) < (size); (bit) = find_next_bit((addr), (size), (bit) + 1))
#define for_each_clear_bit(bit, addr, size) \
	for ((bit) = find_first_zero_bit((addr), (size)); (bit) < (size); (bit) = find_next_zero_bit((addr), (size), (bit) + 1))

#endif /* _LINUXKPI_LINUX_BITOPS_H */
