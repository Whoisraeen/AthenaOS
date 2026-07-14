/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/compiler.h> shim (MPL-2.0, original work).
 *
 * The kernel's compiler-abstraction vocabulary — branch hints, the
 * once-access wrappers, the sparse `__user`/`__iomem` annotations, and the
 * function attributes. Pure macros over standard GCC/Clang features; included
 * almost everywhere. License boundary (../../README.md): API surface, no GPL src.
 */
#ifndef _LINUXKPI_LINUX_COMPILER_H
#define _LINUXKPI_LINUX_COMPILER_H

#define likely(x)    __builtin_expect(!!(x), 1)
#define unlikely(x)  __builtin_expect(!!(x), 0)
#define barrier()    __asm__ __volatile__("" : : : "memory")

/* token paste, two-level so the args are expanded first. Used to build unique
 * labels/identifiers (e.g. drm_exec_until_all_locked's __drm_exec_##__LINE__). */
#ifndef ___PASTE
#define ___PASTE(a, b) a##b
#define __PASTE(a, b)  ___PASTE(a, b)
#endif

/* compile-time type check (SMU's gpu_metrics macros use it); evaluates to 1 but
 * emits a warning if the two types differ. From <linux/typecheck.h>. */
#ifndef typecheck
#define typecheck(type, x) \
	({ type __dummy; __typeof__(x) __dummy2; (void)(&__dummy == &__dummy2); 1; })
#endif

/* once-access: a single volatile load/store the optimizer can't split/fuse. */
#define READ_ONCE(x)      (*(const volatile __typeof__(x) *)&(x))
#define WRITE_ONCE(x, v)  do { *(volatile __typeof__(x) *)&(x) = (v); } while (0)

#define OPTIMIZER_HIDE_VAR(var) __asm__ __volatile__("" : "+r" (var))

/* sparse address-space annotations — no-ops for a normal compile. */
#define __user
#define __kernel
#define __iomem
#define __rcu
#define __percpu
#define __force
#define __bitwise
#define __cond_acquires(x)
#define __acquires(x)
#define __releases(x)
#define __must_hold(x)
#define __private

/* function/variable attributes */
#ifndef __must_check
#define __must_check __attribute__((warn_unused_result))
#endif
#ifndef __maybe_unused
#define __maybe_unused __attribute__((unused))
#endif
#ifndef __always_unused
#define __always_unused __attribute__((unused))
#endif
#ifndef __packed
#define __packed __attribute__((packed))
#endif
#ifndef __aligned
#define __aligned(n) __attribute__((aligned(n)))
#endif
#ifndef __printf
#define __printf(a, b) __attribute__((format(printf, a, b)))
#endif
#ifndef __noreturn
#define __noreturn __attribute__((noreturn))
#endif
#ifndef __cold
#define __cold __attribute__((cold))
#endif
#ifndef __malloc
#define __malloc __attribute__((malloc))
#endif
#ifndef __alloc_size
#define __alloc_size(...) __attribute__((alloc_size(__VA_ARGS__)))
#endif
#ifndef __realloc_size
#define __realloc_size(...) __attribute__((alloc_size(__VA_ARGS__)))
#endif
#ifndef __assume_aligned
#define __assume_aligned(a, ...) __attribute__((assume_aligned(a, ##__VA_ARGS__)))
#endif
#ifndef __always_inline
#define __always_inline inline __attribute__((always_inline))
#endif
#ifndef noinline
#define noinline __attribute__((noinline))
#endif
#ifndef __weak
#define __weak __attribute__((weak))
#endif
#ifndef __deprecated
#define __deprecated __attribute__((deprecated))
#endif
#ifndef fallthrough
#define fallthrough __attribute__((fallthrough))
#endif
#ifndef __nonstring
#define __nonstring
#endif
#ifndef __counted_by
#define __counted_by(m)
#endif

#define typeof_member(T, m) __typeof__(((T *)0)->m)
#define sizeof_field(T, m)  sizeof(((T *)0)->m)

/* flexible-array-member-in-a-union helper (kernel <uapi/linux/stddef.h>). */
#define __DECLARE_FLEX_ARRAY(TYPE, NAME) \
	struct { struct { } __empty_##NAME; TYPE NAME[]; }
#define DECLARE_FLEX_ARRAY(TYPE, NAME) __DECLARE_FLEX_ARRAY(TYPE, NAME)
#define struct_size(p, member, n) \
	(sizeof(*(p)) + sizeof(*(p)->member) * (size_t)(n))

/* current / return instruction pointer (lock + trace diagnostics). Real values
 * via compiler builtins — used only for diagnostic provenance, never control. */
#define _RET_IP_   ((unsigned long)__builtin_return_address(0))
#define _THIS_IP_  ({ __label__ __here; __here: (unsigned long)&&__here; })

#endif /* _LINUXKPI_LINUX_COMPILER_H */
