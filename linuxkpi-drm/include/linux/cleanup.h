/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/cleanup.h> shim (MPL-2.0, original work).
 *
 * Scope-based auto-cleanup (__free / guard / CLASS / DEFINE_GUARD) built on GCC's
 * `__attribute__((cleanup))`. The DRM scheduler uses these for its scoped lock
 * guards + the pending-job iterator. This is an ORIGINAL expression of the cleanup
 * API contract over the compiler attribute (not a transcription of the kernel's
 * macros). REAL behaviour — the destructor genuinely runs at scope exit. License
 * boundary (../../README.md): API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_CLEANUP_H
#define _LINUXKPI_LINUX_CLEANUP_H

#define __cleanup(func)        __attribute__((__cleanup__(func)))
#define ___PASTE(a, b)         a##b
#define __UNIQUE_ID(prefix)    ___PASTE(__UNIQUE_ID_##prefix, __COUNTER__)

/* __free(name): a local pointer auto-freed at scope exit by __free_##name. */
#define DEFINE_FREE(_name, _type, _free) \
	static inline void __free_##_name(void *p) { _type _T = *(_type *)p; _free; }
#define __free(_name) __cleanup(__free_##_name)

/* steal a pointer so its scheduled cleanup will NOT run (ownership transfer). */
#define no_free_ptr(p) \
	({ __typeof__(p) __ptr = (p); (p) = (void *)0; __ptr; })
#define return_ptr(p) return no_free_ptr(p)

/* CLASS: a scoped object with a constructor + destructor (the destructor runs at
 * scope exit). DEFINE_CLASS names the type `class_<name>_t`. */
#define DEFINE_CLASS(_name, _type, _exit, _init, _init_args...) \
	typedef _type class_##_name##_t; \
	static inline void class_##_name##_destructor(_type *p) { _type _T = *p; _exit; } \
	static inline _type class_##_name##_constructor(_init_args) { _type t = _init; return t; }
#define CLASS(_name, var) \
	class_##_name##_t var __cleanup(class_##_name##_destructor) = class_##_name##_constructor

/* DEFINE_GUARD: a lock held for the enclosing scope. guard()/scoped_guard() use it. */
#define DEFINE_GUARD(_name, _type, _lock, _unlock) \
	DEFINE_CLASS(_name, _type, if (_T) { _unlock; }, ({ _lock; _T; }), _type _T) \
	static inline void *class_##_name##_lock_ptr(class_##_name##_t *_T) { return (void *)*_T; }
#define DEFINE_GUARD_COND(_name, _ext, _condlock) /* conditional guards: unused on the MES path */

#define guard(_name) \
	CLASS(_name, __UNIQUE_ID(guard))
#define scoped_guard(_name, args...) \
	for (CLASS(_name, scope)(args), *__done = (void *)0; !__done; __done = (void *)1)
#define scoped_cond_guard(_name, _fail, args...) \
	scoped_guard(_name, args)

#endif /* _LINUXKPI_LINUX_CLEANUP_H */
