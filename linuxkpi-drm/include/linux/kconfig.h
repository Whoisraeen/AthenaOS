/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/kconfig.h> shim (MPL-2.0, original work).
 *
 * The CONFIG-test macros (IS_ENABLED/IS_BUILTIN/IS_MODULE/IS_REACHABLE). The DRM/
 * amdgpu headers gate code with `#if IS_ENABLED(CONFIG_x)`; with a macro
 * undefined those `#if`s otherwise fail to parse ("missing binary operator").
 *
 * The MES bring-up build defines NO CONFIG_* (display/HSA/etc. are out of subset,
 * SCOPE.md), so every IS_*() honestly evaluates to 0 — disabling those guarded
 * blocks, which is exactly the intent. This is the standard kconfig placeholder-
 * token expansion (an original expression of the documented macro contract).
 * License boundary (../../README.md): API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_KCONFIG_H
#define _LINUXKPI_LINUX_KCONFIG_H

#define __ARG_PLACEHOLDER_1  0,
#define __take_second_arg(__ignored, val, ...) val

/* "is this CONFIG token defined as 1?" -> 1, else 0. */
#define __is_defined(x)            ___is_defined(x)
#define ___is_defined(val)         ____is_defined(__ARG_PLACEHOLDER_##val)
#define ____is_defined(arg1_or_junk) __take_second_arg(arg1_or_junk 1, 0)

#define IS_BUILTIN(option)   __is_defined(option)
#define IS_MODULE(option)    __is_defined(option##_MODULE)
#define IS_ENABLED(option)   (IS_BUILTIN(option) || IS_MODULE(option))
/* REACHABLE differs only under modular builds; we have none, so it == ENABLED. */
#define IS_REACHABLE(option) IS_ENABLED(option)

#endif /* _LINUXKPI_LINUX_KCONFIG_H */
