/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/printk.h> shim (MPL-2.0, original work).
 *
 * Kernel logging. amdgpu/DRM emit init progress, warnings, and error diagnostics
 * through printk/pr_*. Backed by ath_linuxkpi's log facade at M4 (Phase 1
 * timing/log). The `pr_*` helpers are macros over printk; KERN_* level prefixes
 * are empty here (the facade carries severity out-of-band). License boundary
 * (../../README.md): API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_PRINTK_H
#define _LINUXKPI_LINUX_PRINTK_H

#include <linux/types.h>
#include <linux/compiler.h>
#include <linux/ratelimit.h>   /* amdgpu embeds ratelimit_state by value (the _rs fields) */
#include <linux/stringify.h>   /* DRM_WARN stringifies source line numbers */

/* level tags concatenate with the format string; the facade tags severity. */
#define KERN_EMERG   ""
#define KERN_ALERT   ""
#define KERN_CRIT    ""
#define KERN_ERR     ""
#define KERN_WARNING ""
#define KERN_NOTICE  ""
#define KERN_INFO    ""
#define KERN_DEBUG   ""
#define KERN_DEFAULT ""
#define KERN_CONT    ""

/* the "%pV" recursive-format wrapper drm_print + dev_printk pass around */
struct va_format {
	const char *fmt;
	va_list    *va;
};

__printf(1, 2) int printk(const char *fmt, ...);
int vprintk(const char *fmt, va_list args);
static inline __printf(1, 2) int no_printk(const char *fmt, ...) { (void)fmt; return 0; }

#define pr_emerg(fmt, ...)   printk(KERN_EMERG fmt, ##__VA_ARGS__)
#define pr_alert(fmt, ...)   printk(KERN_ALERT fmt, ##__VA_ARGS__)
#define pr_crit(fmt, ...)    printk(KERN_CRIT fmt, ##__VA_ARGS__)
#define pr_err(fmt, ...)     printk(KERN_ERR fmt, ##__VA_ARGS__)
#define pr_warn(fmt, ...)    printk(KERN_WARNING fmt, ##__VA_ARGS__)
#define pr_warning(fmt, ...) printk(KERN_WARNING fmt, ##__VA_ARGS__)
#define pr_notice(fmt, ...)  printk(KERN_NOTICE fmt, ##__VA_ARGS__)
#define pr_info(fmt, ...)    printk(KERN_INFO fmt, ##__VA_ARGS__)
#define pr_cont(fmt, ...)    printk(KERN_CONT fmt, ##__VA_ARGS__)
#define pr_debug(fmt, ...)   no_printk(KERN_DEBUG fmt, ##__VA_ARGS__)

/* ratelimited + _once variants collapse to the plain call in the shim. */
#define pr_err_ratelimited(fmt, ...)   pr_err(fmt, ##__VA_ARGS__)
#define pr_warn_ratelimited(fmt, ...)  pr_warn(fmt, ##__VA_ARGS__)
#define pr_info_ratelimited(fmt, ...)  pr_info(fmt, ##__VA_ARGS__)
#define pr_warn_once(fmt, ...)         pr_warn(fmt, ##__VA_ARGS__)
#define pr_info_once(fmt, ...)         pr_info(fmt, ##__VA_ARGS__)
#define pr_err_once(fmt, ...)          pr_err(fmt, ##__VA_ARGS__)
#define printk_once(fmt, ...)          printk(fmt, ##__VA_ARGS__)
#define printk_ratelimited(fmt, ...)   printk(fmt, ##__VA_ARGS__)

/* print_hex_dump prefix_type values (SMU dumps its message tables with these). */
#define DUMP_PREFIX_NONE    0
#define DUMP_PREFIX_ADDRESS 1
#define DUMP_PREFIX_OFFSET  2

void print_hex_dump(const char *level, const char *prefix_str, int prefix_type,
		    int rowsize, int groupsize, const void *buf, size_t len, bool ascii);
void print_hex_dump_debug(const char *prefix_str, int prefix_type, int rowsize,
			  int groupsize, const void *buf, size_t len, bool ascii);

#endif /* _LINUXKPI_LINUX_PRINTK_H */
