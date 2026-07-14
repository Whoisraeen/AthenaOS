/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/stdarg.h> shim (MPL-2.0, original work).
 *
 * Variadic-argument access over the compiler builtins (the kernel's own
 * <linux/stdarg.h> does exactly this). Guarded so it coexists with the va_list
 * already provided by <linux/types.h>. License boundary (../../README.md): surface.
 */
#ifndef _LINUXKPI_LINUX_STDARG_H
#define _LINUXKPI_LINUX_STDARG_H

#ifndef va_start
typedef __builtin_va_list va_list;
#define va_start(ap, last) __builtin_va_start(ap, last)
#define va_end(ap)         __builtin_va_end(ap)
#define va_arg(ap, type)   __builtin_va_arg(ap, type)
#define va_copy(d, s)      __builtin_va_copy(d, s)
#endif

#endif /* _LINUXKPI_LINUX_STDARG_H */
