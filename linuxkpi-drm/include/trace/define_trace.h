/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <trace/define_trace.h> shim (MPL-2.0, original work).
 *
 * In the kernel this re-includes the trace header to emit the tracepoint
 * definitions, but only under CREATE_TRACE_POINTS (one .c file). With tracepoints
 * OFF (see linux/tracepoint.h) it is a no-op — amdgpu_trace.h ends with this
 * include, and for every consumer except the (out-of-subset) trace-points .c it
 * does nothing. License boundary (../../README.md): API surface.
 */
/* intentionally empty */
