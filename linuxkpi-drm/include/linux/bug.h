/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/bug.h> shim (MPL-2.0, original work).
 *
 * Kernel assertions. These are REAL (SCOPE.md rule 9): BUG()/BUG_ON() actually
 * abort the process (a no-op would let a fatal invariant violation continue into
 * undefined behaviour), WARN*() really log and return the condition (so callers
 * branch on it correctly), and BUILD_BUG_ON() is a genuine compile-time check.
 * License boundary (../../README.md): API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_BUG_H
#define _LINUXKPI_LINUX_BUG_H

#include <linux/types.h>
#include <linux/compiler.h>
#include <linux/printk.h>

/* fatal: trap (SIGILL) — never silently continue. */
#define BUG() do { printk("BUG at %s:%d\n", __FILE__, __LINE__); __builtin_trap(); } while (0)
#define BUG_ON(cond) do { if (unlikely(cond)) BUG(); } while (0)

/* non-fatal: log once-ish and RETURN the condition so the caller can react. */
#define WARN_ON(cond) ({ \
	int __wc = !!(cond); \
	if (unlikely(__wc)) printk("WARNING at %s:%d\n", __FILE__, __LINE__); \
	__wc; })
#define WARN_ON_ONCE(cond) WARN_ON(cond)
#define WARN(cond, fmt, ...) ({ \
	int __wc = !!(cond); \
	if (unlikely(__wc)) printk("WARNING: " fmt, ##__VA_ARGS__); \
	__wc; })
#define WARN_ONCE(cond, fmt, ...) WARN(cond, fmt, ##__VA_ARGS__)

/* kernel taint flags + add_taint() — a diagnostic marker; amdgpu taints on a GPU
 * hang/reset. Honest no-op (the daemon's own crash log carries this), not a fake
 * of a functional path. */
#define TAINT_PROPRIETARY_MODULE  0
#define TAINT_WARN                9
#define TAINT_SOFTLOCKUP         14
#define TAINT_MACHINE_CHECK       4
#define LOCKDEP_STILL_OK          1
#define LOCKDEP_NOW_UNRELIABLE    0
#define add_taint(flag, lockdep_ok) do { (void)(flag); (void)(lockdep_ok); } while (0)

/* compile-time assertions */
#define BUILD_BUG_ON(cond)            _Static_assert(!(cond), "BUILD_BUG_ON: " #cond)
#define BUILD_BUG_ON_MSG(cond, msg)   _Static_assert(!(cond), msg)
#define BUILD_BUG_ON_ZERO(e)          (sizeof(struct { int:(-!!(e)); }))
/* type-checks the expression without emitting code (drm_mm.h uses it in the
 * DRM_MM_BUG_ON wrapper when debug is off). */
#define BUILD_BUG_ON_INVALID(e)       ((void)(sizeof((long)(e))))
#define BUILD_BUG()                   _Static_assert(0, "BUILD_BUG")
#define static_assert(expr, ...)      _Static_assert(expr, #expr)

#endif /* _LINUXKPI_LINUX_BUG_H */
