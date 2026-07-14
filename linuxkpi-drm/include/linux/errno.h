/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/errno.h> shim (MPL-2.0, original work).
 *
 * The errno constants (POSIX values + the kernel-internal ones like ERESTARTSYS).
 * amdgpu returns `-EINVAL`/`-ENOMEM`/`-EBUSY`/... pervasively, and the SCOPE rule
 * is honest error returns — never a faked 0 (rule 9), so these values are
 * load-bearing. Standard ABI values. License boundary (../../README.md): surface.
 */
#ifndef _LINUXKPI_LINUX_ERRNO_H
#define _LINUXKPI_LINUX_ERRNO_H

#define EPERM            1
#define ENOENT           2
#define ESRCH            3
#define EINTR            4
#define EIO              5
#define ENXIO            6
#define E2BIG            7
#define ENOEXEC          8
#define EBADF            9
#define ECHILD          10
#define EAGAIN          11
#define ENOMEM          12
#define EACCES          13
#define EFAULT          14
#define EBUSY           16
#define EEXIST          17
#define ENODEV          19
#define ENOTDIR         20
#define EISDIR          21
#define EINVAL          22
#define ENFILE          23
#define EMFILE          24
#define ENOTTY          25
#define ENOSPC          28
#define ESPIPE          29
#define EROFS           30
#define EPIPE           32
#define ERANGE          34
#define EDEADLK         35
#define ENAMETOOLONG    36
#define ENOSYS          38
#define ENOTEMPTY       39
#define ELOOP           40
#define ECANCELED      125
#define ENODATA         61
#define ETIME           62
#define EPROTO          71
#define EOVERFLOW       75
#define EBADMSG         74
#define ENOTSUPP        524
#define EOPNOTSUPP      95
#define ENOTSOCK        88
#define EMSGSIZE        90
#define EPROTONOSUPPORT 93
#define EAFNOSUPPORT    97
#define EADDRINUSE      98
#define ENETUNREACH    101
#define ECONNRESET     104
#define ETIMEDOUT      110
#define ECONNREFUSED   111
#define EHOSTUNREACH   113
#define EALREADY       114
#define EINPROGRESS    115
#define EREMOTEIO      121
#define ENOMEDIUM      123
#define EMULTIHOP      72
#define EHWPOISON      133
#define EWOULDBLOCK    EAGAIN

/* kernel-internal "should not escape to userspace" codes */
#define ERESTARTSYS     512
#define ERESTARTNOINTR  513
#define ERESTARTNOHAND  514
#define ENOIOCTLCMD     515
#define ERESTART_RESTARTBLOCK 516
#define EPROBE_DEFER    517

#endif /* _LINUXKPI_LINUX_ERRNO_H */
