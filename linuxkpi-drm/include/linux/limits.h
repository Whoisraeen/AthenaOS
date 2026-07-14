/* SPDX-License-Identifier: MPL-2.0 */
/* <linux/limits.h> shim (MPL-2.0): the U*_MAX/S*_MAX/INT_MAX family lives in
 * <linux/types.h>; this re-exports it so explicit includers resolve here, not the
 * host header. License boundary (../../README.md): API surface. */
#ifndef _LINUXKPI_LINUX_LIMITS_H
#define _LINUXKPI_LINUX_LIMITS_H
#include <linux/types.h>
#define PATH_MAX 4096
#define NAME_MAX 255
#endif
