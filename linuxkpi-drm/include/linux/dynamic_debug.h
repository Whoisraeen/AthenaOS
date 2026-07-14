/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/dynamic_debug.h> shim (MPL-2.0, original work).
 *
 * Runtime-toggled debug printing. drm_print.h pulls it in for drm_dbg/pr_debug
 * call-site descriptors. We build with dynamic debug OFF (the upstream
 * CONFIG_DYNAMIC_DEBUG=n posture — a legitimate config, not a fake): the debug
 * emitters compile away and the class maps are empty. Normal-severity logging is
 * unaffected (it goes through printk.h). License boundary: API surface only.
 */
#ifndef _LINUXKPI_LINUX_DYNAMIC_DEBUG_H
#define _LINUXKPI_LINUX_DYNAMIC_DEBUG_H

#include <linux/printk.h>

struct _ddebug;
struct device;

#define DECLARE_DYNDBG_CLASSMAP(_name, _type, _base, ...)
#define DEFINE_DYNAMIC_DEBUG_METADATA(name, fmt)
#define DYNAMIC_DEBUG_BRANCH(descriptor) (0)

#define __dynamic_pr_debug(desc, fmt, ...)        no_printk(fmt, ##__VA_ARGS__)
#define dynamic_pr_debug(fmt, ...)                no_printk(fmt, ##__VA_ARGS__)
#define dynamic_dev_dbg(dev, fmt, ...)            no_printk(fmt, ##__VA_ARGS__)
#define __dynamic_dev_dbg(desc, dev, fmt, ...)    no_printk(fmt, ##__VA_ARGS__)
#define _dynamic_func_call(fmt, func, ...)        do { } while (0)
#define _dynamic_func_call_no_desc(fmt, func, ...) do { } while (0)

#endif /* _LINUXKPI_LINUX_DYNAMIC_DEBUG_H */
