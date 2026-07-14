/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/string_helpers.h> shim (MPL-2.0, original work).
 *
 * The small str_*() boolean-to-word helpers the amdgpu ATOMBIOS interpreter
 * (atom.c) and a few other files use in debug/log strings. Trivial constant
 * expressions of the documented kernel contract, not GPL source. License
 * boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_STRING_HELPERS_H
#define _LINUXKPI_LINUX_STRING_HELPERS_H

#include <linux/types.h>

static inline const char *str_yes_no(bool v)        { return v ? "yes" : "no"; }
static inline const char *str_no_yes(bool v)        { return v ? "no" : "yes"; }
static inline const char *str_on_off(bool v)        { return v ? "on" : "off"; }
static inline const char *str_enable_disable(bool v){ return v ? "enable" : "disable"; }
static inline const char *str_enabled_disabled(bool v){ return v ? "enabled" : "disabled"; }
static inline const char *str_true_false(bool v)    { return v ? "true" : "false"; }

#endif /* _LINUXKPI_LINUX_STRING_HELPERS_H */
