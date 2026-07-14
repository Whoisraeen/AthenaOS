/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/err.h> shim (MPL-2.0, original work).
 *
 * The kernel's "error pointer" idiom: small negative errno values encoded in a
 * pointer's high range, so a function can return either a valid pointer or
 * -EXXX. amdgpu uses IS_ERR/PTR_ERR/ERR_PTR pervasively. These MUST be inline —
 * if a TU reaches IS_ERR() without this header they compile as an implicit
 * function call and become an undefined `IS_ERR` symbol at link, so err.h is
 * pulled from the universal base (types.h). License boundary: API surface.
 */
#ifndef _LINUXKPI_LINUX_ERR_H
#define _LINUXKPI_LINUX_ERR_H

#include <linux/types.h>

#define MAX_ERRNO 4095
#define IS_ERR_VALUE(x) ((unsigned long)(void *)(x) >= (unsigned long)-MAX_ERRNO)

static inline void *ERR_PTR(long error)            { return (void *)error; }
static inline long  PTR_ERR(const void *ptr)       { return (long)ptr; }
static inline bool  IS_ERR(const void *ptr)        { return IS_ERR_VALUE((unsigned long)ptr); }
static inline bool  IS_ERR_OR_NULL(const void *ptr){ return !ptr || IS_ERR_VALUE((unsigned long)ptr); }
static inline void *ERR_CAST(const void *ptr)      { return (void *)ptr; }
static inline int   PTR_ERR_OR_ZERO(const void *ptr){ return IS_ERR(ptr) ? (int)PTR_ERR(ptr) : 0; }

#endif /* _LINUXKPI_LINUX_ERR_H */
