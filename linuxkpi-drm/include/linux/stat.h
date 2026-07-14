/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/stat.h> shim (MPL-2.0, original work).
 *
 * File-mode permission bits. amdgpu spells its sysfs attribute modes with these
 * (S_IWUSR etc. in DEVICE_ATTR). Standard octal POSIX values. License boundary
 * (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_STAT_H
#define _LINUXKPI_LINUX_STAT_H

#define S_IRWXU 0700
#define S_IRUSR 0400
#define S_IWUSR 0200
#define S_IXUSR 0100
#define S_IRWXG 0070
#define S_IRGRP 0040
#define S_IWGRP 0020
#define S_IXGRP 0010
#define S_IRWXO 0007
#define S_IROTH 0004
#define S_IWOTH 0002
#define S_IXOTH 0001
#define S_IRUGO (S_IRUSR | S_IRGRP | S_IROTH)
#define S_IWUGO (S_IWUSR | S_IWGRP | S_IWOTH)
#define S_IXUGO (S_IXUSR | S_IXGRP | S_IXOTH)

#endif /* _LINUXKPI_LINUX_STAT_H */
