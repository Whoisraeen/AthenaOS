/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/unaligned.h> shim (MPL-2.0, original work).
 *
 * Byte-assembled unaligned little-endian accessors the amdgpu ATOMBIOS
 * interpreter (atom.c) uses to read the VBIOS image. Portable expressions of
 * the documented kernel contract, not GPL source. License boundary
 * (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_UNALIGNED_H
#define _LINUXKPI_LINUX_UNALIGNED_H

#include <linux/types.h>

static inline u16 get_unaligned_le16(const void *p)
{
	const u8 *b = p;
	return (u16)b[0] | ((u16)b[1] << 8);
}
static inline u32 get_unaligned_le32(const void *p)
{
	const u8 *b = p;
	return (u32)b[0] | ((u32)b[1] << 8) | ((u32)b[2] << 16) | ((u32)b[3] << 24);
}
static inline u64 get_unaligned_le64(const void *p)
{
	const u8 *b = p;
	return (u64)get_unaligned_le32(b) | ((u64)get_unaligned_le32(b + 4) << 32);
}
static inline void put_unaligned_le16(u16 v, void *p)
{
	u8 *b = p;
	b[0] = (u8)v;
	b[1] = (u8)(v >> 8);
}
static inline void put_unaligned_le32(u32 v, void *p)
{
	u8 *b = p;
	b[0] = (u8)v;
	b[1] = (u8)(v >> 8);
	b[2] = (u8)(v >> 16);
	b[3] = (u8)(v >> 24);
}

#endif /* _LINUXKPI_LINUX_UNALIGNED_H */
