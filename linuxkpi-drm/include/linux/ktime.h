/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/ktime.h> shim (MPL-2.0, original work).
 *
 * The kernel's nanosecond-resolution scalar time API. `ktime_t` is a signed
 * 64-bit nanosecond count (declared in <linux/types.h>); amdgpu uses ktime for
 * fence/submit timeouts, GPU-reset timing, and delay loops.
 *
 * License boundary (../../README.md): this declares the ktime *API surface*
 * (signatures + the arithmetic the function names dictate), NOT GPL source. The
 * clock READERS need a real monotonic source, so they are declaration-only and
 * resolve to ath_linuxkpi at link time (M4). The arithmetic/comparison helpers
 * are pure s64 math — the only meaning their names can carry — so they are
 * defined inline here. No silent-success fakes (SCOPE.md).
 */
#ifndef _LINUXKPI_LINUX_KTIME_H
#define _LINUXKPI_LINUX_KTIME_H

#include <linux/types.h>

/* Time-unit scaling constants (kernel-wide; guarded so a later <linux/time*.h>
 * shim can define them too without clashing). */
#ifndef NSEC_PER_SEC
#define NSEC_PER_SEC   1000000000L
#endif
#ifndef NSEC_PER_MSEC
#define NSEC_PER_MSEC  1000000L
#endif
#ifndef NSEC_PER_USEC
#define NSEC_PER_USEC  1000L
#endif
#ifndef USEC_PER_SEC
#define USEC_PER_SEC   1000000L
#endif
#ifndef USEC_PER_MSEC
#define USEC_PER_MSEC  1000L
#endif
#ifndef MSEC_PER_SEC
#define MSEC_PER_SEC   1000L
#endif

/* Largest representable ktime_t (saturating sentinel for "never times out"). */
#define KTIME_MAX  ((s64)~((u64)1 << 63))

/* ---- clock readers (declaration-only; ath_linuxkpi backs these at M4) ----
 * A real monotonic/wall source is required, so these cannot be inlined here. */
ktime_t ktime_get(void);              /* CLOCK_MONOTONIC */
ktime_t ktime_get_real(void);         /* CLOCK_REALTIME  */
ktime_t ktime_get_boottime(void);     /* monotonic incl. suspend */
u64     ktime_get_ns(void);
u64     ktime_get_real_ns(void);
u64     ktime_get_boottime_ns(void);
u64     ktime_get_mono_fast_ns(void);

/* ---- pure arithmetic (the only meaning the names carry) ---- */
static inline ktime_t ktime_set(const s64 secs, const unsigned long nsecs)
{
	return (ktime_t)secs * NSEC_PER_SEC + (s64)nsecs;
}
static inline s64     ktime_to_ns(const ktime_t kt)           { return kt; }
static inline ktime_t ns_to_ktime(u64 ns)                     { return (ktime_t)ns; }
static inline ktime_t ktime_add(const ktime_t a, const ktime_t b) { return a + b; }
static inline ktime_t ktime_sub(const ktime_t a, const ktime_t b) { return a - b; }
static inline ktime_t ktime_add_ns(const ktime_t kt, const u64 ns) { return kt + (s64)ns; }
static inline ktime_t ktime_sub_ns(const ktime_t kt, const u64 ns) { return kt - (s64)ns; }
static inline ktime_t ktime_add_us(const ktime_t kt, const u64 us) { return kt + (s64)(us * NSEC_PER_USEC); }
static inline ktime_t ktime_add_ms(const ktime_t kt, const u64 ms) { return kt + (s64)(ms * NSEC_PER_MSEC); }
static inline s64     ktime_to_us(const ktime_t kt)           { return kt / NSEC_PER_USEC; }
static inline s64     ktime_to_ms(const ktime_t kt)           { return kt / NSEC_PER_MSEC; }
static inline s64     ktime_us_delta(const ktime_t later, const ktime_t earlier) { return (later - earlier) / NSEC_PER_USEC; }
static inline s64     ktime_ms_delta(const ktime_t later, const ktime_t earlier) { return (later - earlier) / NSEC_PER_MSEC; }

/* ---- comparison (s64 ordering of nanosecond counts) ---- */
static inline int ktime_compare(const ktime_t a, const ktime_t b)
{
	if (a < b)
		return -1;
	if (a > b)
		return 1;
	return 0;
}
static inline bool ktime_after(const ktime_t a, const ktime_t b)  { return ktime_compare(a, b) > 0; }
static inline bool ktime_before(const ktime_t a, const ktime_t b) { return ktime_compare(a, b) < 0; }
static inline bool ktime_equal(const ktime_t a, const ktime_t b)  { return a == b; }

#endif /* _LINUXKPI_LINUX_KTIME_H */
