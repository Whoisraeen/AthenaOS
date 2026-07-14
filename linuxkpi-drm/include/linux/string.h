/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/string.h> shim (MPL-2.0, original work).
 *
 * The mem and str helper family. The core ones (memcpy/memset/memmove/strlen/...) are
 * provided by raeen_linuxkpi's libc-shadow at link time, declared here. memset32/
 * memset64 (word fills) are pure inlines. Pulled from <linux/types.h> so these
 * are available wherever a header uses them transitively (kernel parity). License
 * boundary (../../README.md): API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_STRING_H
#define _LINUXKPI_LINUX_STRING_H

#include <linux/types.h>

/* provided by raeen_linuxkpi (or the compiler builtins) at link time */
void  *memcpy(void *dst, const void *src, size_t n);
void  *memmove(void *dst, const void *src, size_t n);
void  *memset(void *s, int c, size_t n);
int    memcmp(const void *a, const void *b, size_t n);
void  *memchr(const void *s, int c, size_t n);
size_t strlen(const char *s);
size_t strnlen(const char *s, size_t maxlen);
char  *strcpy(char *dst, const char *src);
char  *strncpy(char *dst, const char *src, size_t n);
int    strcmp(const char *a, const char *b);
int    strncmp(const char *a, const char *b, size_t n);
char  *strchr(const char *s, int c);
char  *strstr(const char *h, const char *n);
char  *strsep(char **s, const char *ct);
char  *kstrdup(const char *s, gfp_t gfp);
ssize_t strscpy(char *dst, const char *src, size_t count);

/* word fills — pure inlines (the only meaning the names carry). */
static inline void *memset32(u32 *s, u32 v, size_t count) { u32 *p = s; while (count--) *p++ = v; return s; }
static inline void *memset64(u64 *s, u64 v, size_t count) { u64 *p = s; while (count--) *p++ = v; return s; }

#endif /* _LINUXKPI_LINUX_STRING_H */
