/* SPDX-License-Identifier: MPL-2.0 */
/* <linux/kstrtox.h> shim (MPL-2.0): string-to-number parsing. amdgpu parses sysfs
 * writes with these. Backed by ath_linuxkpi at M4 (real parse; a fake that
 * always returned 0 would mis-read user input -- rule 9). License boundary: surface. */
#ifndef _LINUXKPI_LINUX_KSTRTOX_H
#define _LINUXKPI_LINUX_KSTRTOX_H
#include <linux/types.h>
int kstrtoull(const char *s, unsigned int base, unsigned long long *res);
int kstrtoll(const char *s, unsigned int base, long long *res);
int kstrtoul(const char *s, unsigned int base, unsigned long *res);
int kstrtol(const char *s, unsigned int base, long *res);
int kstrtouint(const char *s, unsigned int base, unsigned int *res);
int kstrtoint(const char *s, unsigned int base, int *res);
int kstrtobool(const char *s, bool *res);
#endif
