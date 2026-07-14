/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/efi.h> shim (MPL-2.0, original work).
 *
 * EFI firmware interface. amdgpu_device checks efi_enabled(EFI_BOOT) and can pull
 * the VBIOS image from an EFI variable. On Athena the VBIOS is obtained via the
 * normal ROM/IP-discovery path, so efi_enabled is backed by ath_linuxkpi at M4
 * (reports the real boot mode); the variable read is M4 too. Minimal surface for
 * the type/decl layout. License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_EFI_H
#define _LINUXKPI_LINUX_EFI_H

#include <linux/types.h>

#define EFI_BOOT              1
#define EFI_RUNTIME_SERVICES  3
#define EFI_MEMMAP            4
#define EFI_64BIT            10

/* report the real EFI feature state — backed by ath_linuxkpi (M4) */
bool efi_enabled(int feature);

#endif /* _LINUXKPI_LINUX_EFI_H */
