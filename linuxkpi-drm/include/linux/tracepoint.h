/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/tracepoint.h> shim (MPL-2.0, original work).
 *
 * Static tracepoints. amdgpu_trace.h defines a TRACE_EVENT for each trace_amdgpu_*
 * call site. We build with tracepoints OFF (the upstream !CONFIG_TRACEPOINTS
 * posture — a legitimate config): each TRACE_EVENT/DEFINE_EVENT generates an EMPTY
 * static-inline `trace_<name>(...)` with the correct prototype, so every
 * `trace_amdgpu_xxx(args)` call site compiles to a no-op. This is honest
 * tracing-disabled, not a fake of a functional path. License boundary
 * (../../README.md): API surface, no GPL source.
 */
#ifndef _LINUXKPI_LINUX_TRACEPOINT_H
#define _LINUXKPI_LINUX_TRACEPOINT_H

#include <linux/types.h>

/* TP_* helpers: PROTO/ARGS pass through; the body descriptors expand to nothing. */
#define TP_PROTO(args...)            args
#define TP_ARGS(args...)             args
#define TP_STRUCT__entry(args...)
#define TP_fast_assign(args...)
#define TP_printk(fmt, args...)
#define TP_CONDITION(args...)
#define TP_perf_assign(args...)

/* Each event generates a no-op emit inline + a `_enabled` predicate (always
 * false — tracing is compiled out) with the declared prototype. amdgpu guards
 * work with `if (trace_<name>_enabled())`. */
#define TRACE_EVENT(name, proto, args, tstruct, assign, print) \
	static inline void trace_##name(proto) { } \
	static inline bool trace_##name##_enabled(void) { return false; }
#define TRACE_EVENT_CONDITION(name, proto, args, cond, tstruct, assign, print) \
	static inline void trace_##name(proto) { } \
	static inline bool trace_##name##_enabled(void) { return false; }
#define DECLARE_EVENT_CLASS(name, proto, args, tstruct, assign, print)
#define DEFINE_EVENT(template, name, proto, args) \
	static inline void trace_##name(proto) { } \
	static inline bool trace_##name##_enabled(void) { return false; }
#define DEFINE_EVENT_PRINT(template, name, proto, args, print) \
	static inline void trace_##name(proto) { } \
	static inline bool trace_##name##_enabled(void) { return false; }
#define TRACE_EVENT_FN(name, proto, args, tstruct, assign, print, reg, unreg) \
	static inline void trace_##name(proto) { } \
	static inline bool trace_##name##_enabled(void) { return false; }

#define DECLARE_TRACE(name, proto, args) \
	static inline void trace_##name(proto) { } \
	static inline bool trace_##name##_enabled(void) { return false; }
#define DEFINE_TRACE(name, proto, args)
#define EXPORT_TRACEPOINT_SYMBOL(name)
#define EXPORT_TRACEPOINT_SYMBOL_GPL(name)

#define TRACE_INCLUDE(file)
#define TRACE_INCLUDE_PATH(p)
#define TRACE_INCLUDE_FILE(f)

#endif /* _LINUXKPI_LINUX_TRACEPOINT_H */
