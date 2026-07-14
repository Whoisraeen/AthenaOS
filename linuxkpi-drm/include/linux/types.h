/* SPDX-License-Identifier: MPL-2.0 */
/*
 * linuxkpi-drm — LinuxKPI C header shim (MPL-2.0, original work).
 *
 * <linux/types.h>: the Linux kernel integer/type vocabulary mapped onto the C
 * standard library. This is the canonical first shim header the amdgpu driver
 * reaches for. It declares *signatures/types* (Linux API surface), not GPL
 * kernel source — see ../../README.md for the license boundary.
 *
 * Error-driven: types are added here only when a real compile demands them
 * (SCOPE.md). Keep it honest — no fakes, no silent-success typedefs.
 */
#ifndef _LINUXKPI_LINUX_TYPES_H
#define _LINUXKPI_LINUX_TYPES_H

#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
/* the CONFIG-test macros are used in `#if IS_ENABLED(...)` across the headers;
 * make them available from this universal base so those #ifs parse. */
#include <linux/kconfig.h>
/* errno is effectively ubiquitous in the kernel headers (drm/ttm use bare EXXX in
 * inline helpers expecting it transitively); pull it from this universal base. */
#include <linux/errno.h>

/* Sparse address-space annotations — no-ops for a normal compile. Defined here
 * (not only in <linux/compiler.h>) because the uapi headers reach for `__user`/
 * `__force` having included only <linux/types.h>. Guarded so compiler.h coexists. */
#ifndef __user
#define __user
#endif
#ifndef __kernel
#define __kernel
#endif
#ifndef __iomem
#define __iomem
#endif
#ifndef __force
#define __force
#endif
#ifndef __bitwise
#define __bitwise
#endif

typedef uint8_t  u8;
typedef uint16_t u16;
typedef uint32_t u32;
typedef uint64_t u64;
typedef int8_t   s8;
typedef int16_t  s16;
typedef int32_t  s32;
typedef int64_t  s64;

/* BIT/GENMASK live here (the dependency-free base) so they are constant-folded
 * everywhere a header uses them transitively (e.g. an enum = BIT(31)); bits.h /
 * bitops.h re-expose them, guarded. */
#ifndef BITS_PER_BYTE
#define BITS_PER_BYTE      8
#define BITS_PER_LONG      64
#define BITS_PER_LONG_LONG 64
#define BIT(nr)        (1UL << (nr))
#define BIT_ULL(nr)    (1ULL << (nr))
#define GENMASK(h, l)     (((~0UL) << (l)) & (~0UL >> (BITS_PER_LONG - 1 - (h))))
#define GENMASK_ULL(h, l) (((~0ULL) << (l)) & (~0ULL >> (BITS_PER_LONG_LONG - 1 - (h))))
#endif

typedef u8  __u8;
typedef u16 __u16;
typedef u32 __u32;
typedef u64 __u64;
typedef s8  __s8;
typedef s16 __s16;
typedef s32 __s32;
typedef s64 __s64;

/* 64-bit-aligned uapi integers (the kernel forces 8-byte alignment in uapi structs
 * so 32/64-bit userspace agree on layout; on x86_64 a plain u64 is already
 * 8-aligned). */
typedef u64 __attribute__((aligned(8))) __aligned_u64;
typedef u64 __attribute__((aligned(8))) __aligned_be64;
typedef u64 __attribute__((aligned(8))) __aligned_le64;

/* endian conversions — x86_64 is little-endian, so the le<->cpu forms are identity
 * (macros, so they replace the call sites and never become undefined symbols). */
#define cpu_to_le16(x) ((__le16)(u16)(x))
#define cpu_to_le32(x) ((__le32)(u32)(x))
#define cpu_to_le64(x) ((__le64)(u64)(x))
#define le16_to_cpu(x) ((u16)(__le16)(x))
#define le32_to_cpu(x) ((u32)(__le32)(x))
#define le64_to_cpu(x) ((u64)(__le64)(x))
#define le16_to_cpup(p) le16_to_cpu(*(const __le16 *)(p))
#define le32_to_cpup(p) le32_to_cpu(*(const __le32 *)(p))
#define le64_to_cpup(p) le64_to_cpu(*(const __le64 *)(p))
#define cpu_to_be32(x)  ((__be32)__builtin_bswap32((u32)(x)))
#define be32_to_cpu(x)  ((u32)__builtin_bswap32((u32)(__be32)(x)))

/* endian-annotated aliases (the kernel uses these as plain integers on x86_64) */
typedef u16 __le16;
typedef u32 __le32;
typedef u64 __le64;
typedef u16 __be16;
typedef u32 __be32;
typedef u64 __be64;

typedef u64 dma_addr_t;
typedef u64 phys_addr_t;
typedef u64 resource_size_t;
typedef s64 ktime_t;

/* __kernel_* UAPI scalar types (LP64) — the uapi/drm/*.h headers spell their
 * fields with these. Mapped to the x86_64 LP64 widths. */
typedef unsigned long  __kernel_size_t;
typedef long           __kernel_ssize_t;
typedef long           __kernel_long_t;
typedef unsigned long  __kernel_ulong_t;
typedef long           __kernel_off_t;
typedef long long      __kernel_loff_t;
typedef int            __kernel_pid_t;
typedef unsigned int   __kernel_uid32_t;
typedef unsigned int   __kernel_gid32_t;
typedef long long      __kernel_time64_t;
typedef int            __kernel_clockid_t;

/* poll event mask + 128-bit GUID (used widely; the DP-MST topology keys on guid).
 * Named struct so the typedef is identical wherever re-declared (uuid.h). */
typedef unsigned int   __poll_t;
struct __linuxkpi_guid { __u8 b[16]; };
typedef struct __linuxkpi_guid guid_t;
typedef struct __linuxkpi_guid uuid_t;

typedef unsigned char  uchar;
typedef unsigned short ushort;
typedef unsigned int   uint;
typedef unsigned long  ulong;
typedef unsigned int   gfp_t;
typedef unsigned int   fmode_t;
typedef unsigned long  pgoff_t;
typedef unsigned short umode_t;
typedef int            pid_t;
typedef __kernel_ssize_t ssize_t;
typedef __kernel_loff_t  loff_t;
typedef u32              dev_t;
typedef _Bool          bool_t;

/* varargs (the kernel uses __builtin_va_* directly; drm_print spells va_list). */
typedef __builtin_va_list va_list;
#ifndef va_start
#define va_start(ap, last) __builtin_va_start(ap, last)
#define va_end(ap)         __builtin_va_end(ap)
#define va_arg(ap, type)   __builtin_va_arg(ap, type)
#define va_copy(d, s)      __builtin_va_copy(d, s)
#endif

/* integer limits (kernel <linux/limits.h> surface; the DRM/amdgpu headers use the
 * U*_MAX/S*_MAX spellings as sentinels). */
#ifndef U8_MAX
#define U8_MAX   ((u8)~0U)
#define U16_MAX  ((u16)~0U)
#define U32_MAX  ((u32)~0U)
#define U64_MAX  ((u64)~0ULL)
#define S8_MAX   ((s8)(U8_MAX >> 1))
#define S16_MAX  ((s16)(U16_MAX >> 1))
#define S32_MAX  ((s32)(U32_MAX >> 1))
#define S64_MAX  ((s64)(U64_MAX >> 1))
#define S32_MIN  ((s32)(-S32_MAX - 1))
#define S64_MIN  ((s64)(-S64_MAX - 1))
#define INT_MAX  ((int)(~0U >> 1))
#define INT_MIN  (-INT_MAX - 1)
#define UINT_MAX (~0U)
#define LONG_MAX ((long)(~0UL >> 1))
#define ULONG_MAX (~0UL)
#define SIZE_MAX (~(size_t)0)
#endif

#ifndef NUMA_NO_NODE
#define NUMA_NO_NODE (-1)
#endif

/* uapi pointer helpers — tiny and used in ioctl arg handling across files that
 * don't all include <linux/uaccess.h>; define them on the base (need __user). */
#ifndef u64_to_user_ptr
#define u64_to_user_ptr(x) ((void __user *)(uintptr_t)(x))
#endif
#ifndef untagged_addr
#define untagged_addr(addr) (addr)
#endif

/* global system run-state (amdgpu checks it during shutdown/reset). */
enum system_states {
	SYSTEM_BOOTING, SYSTEM_SCHEDULING, SYSTEM_FREEING_INITMEM, SYSTEM_RUNNING,
	SYSTEM_HALT, SYSTEM_POWER_OFF, SYSTEM_RESTART, SYSTEM_SUSPEND,
};
extern enum system_states system_state;

#ifndef DMA_BIT_MASK
#define DMA_BIT_MASK(n) (((n) == 64) ? ~0ULL : ((1ULL << (n)) - 1))
#endif

struct list_head {
	struct list_head *next, *prev;
};

struct hlist_head {
	struct hlist_node *first;
};
struct hlist_node {
	struct hlist_node *next, **pprev;
};

/* bit ops (and, transitively, atomics) are ubiquitous in the kernel — many DRM/
 * amdgpu headers call test_bit/set_bit from inlines WITHOUT including bitops.h,
 * relying on it being transitively present. Pull it from this universal base (at
 * the END, after every type above is defined) so those uses see the real static
 * inline rather than an implicit non-static declaration (which would conflict). */
#include <linux/bitops.h>
/* jiffies/HZ + time_after() are ubiquitous in amdgpu/SMU timeout loops, used from
 * inlines without including jiffies.h; pull it here (it depends only on this
 * header). */
#include <linux/jiffies.h>
/* list_empty/list_add etc. are likewise called from headers that don't include
 * list.h (relying on kernel transitivity); pull it so those see the real static
 * inline, not an implicit non-static decl. SZ_* size constants are equally
 * ubiquitous (aperture/BAR sizing). Both depend only on this header. */
#include <linux/list.h>
#include <linux/sizes.h>
/* the compiler-attribute vocabulary (__must_check/__cold/__rcu/__always_inline
 * etc.) is force-included into every kernel TU; many DRM headers spell these in
 * declarations without including <linux/compiler.h>, so pull it from the base. */
#include <linux/compiler.h>
/* the MODULE_*() metadata macros (MODULE_FIRMWARE/LICENSE/PARM/...) are no-ops
 * spread across nearly every driver .c, often reached transitively (a file uses
 * MODULE_FIRMWARE without including <linux/module.h>). Pull the light macro header
 * from the base so a bare MODULE_FIRMWARE("...") never falls through as a stray
 * string at file scope. module.h includes only this header (guarded — no cycle). */
#include <linux/module.h>
/* mem and str helpers + memset32 are used from headers without including string.h. */
#include <linux/string.h>
/* the ERR_PTR/IS_ERR/PTR_ERR error-pointer idiom is ubiquitous and MUST be inline
 * (an un-included use compiles as an implicit call -> undefined symbol at link);
 * pull it from the base. err.h includes only this header (guarded — no cycle). */
#include <linux/err.h>
/* overflow-checked size arithmetic (struct_size/array_size/check_*_overflow) and
 * the 64-bit division helpers (do_div/div_u64) are likewise used from headers and
 * inlines without an explicit include; both are light and depend only on this
 * base, so pull them universally to avoid implicit-call -> undefined-symbol. */
#include <linux/overflow.h>
#include <linux/math64.h>
/* BUILD_BUG_ON/WARN_ON are sprinkled across nearly every driver .c and are macros
 * (a missed include makes WARN_ON an implicit call -> undefined symbol); pull the
 * light bug.h from the base. */
#include <linux/bug.h>
/* struct inode/file are reached transitively (drm_device.anon_inode->i_mapping). */
#include <linux/fs.h>

#endif /* _LINUXKPI_LINUX_TYPES_H */
