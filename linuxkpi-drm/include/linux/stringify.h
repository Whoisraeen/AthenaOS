/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/stringify.h> shim (MPL-2.0, original work).
 *
 * Two-level token stringification (so macro args expand before being quoted).
 * amdgpu's tracepoint + firmware-name machinery uses it. Pure preprocessor.
 * License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_STRINGIFY_H
#define _LINUXKPI_LINUX_STRINGIFY_H

#define __stringify_1(x...) #x
#define __stringify(x...)   __stringify_1(x)

#endif /* _LINUXKPI_LINUX_STRINGIFY_H */
