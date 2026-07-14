/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/export.h> shim (MPL-2.0, original work).
 *
 * EXPORT_SYMBOL marks a kernel symbol as visible to other modules. In the
 * AthenaOS link model the whole amdgpu object set links against raeen_linuxkpi as
 * one unit, so the export annotation is a compile-time no-op — every symbol is
 * already in scope. License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_EXPORT_H
#define _LINUXKPI_LINUX_EXPORT_H

#include <linux/stringify.h>

#define EXPORT_SYMBOL(sym)
#define EXPORT_SYMBOL_GPL(sym)
#define EXPORT_SYMBOL_NS(sym, ns)
#define EXPORT_SYMBOL_NS_GPL(sym, ns)
#define EXPORT_SYMBOL_FOR_MODULES(sym, mods)

#endif /* _LINUXKPI_LINUX_EXPORT_H */
