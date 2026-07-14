/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <kunit/test-bug.h> shim (MPL-2.0, original work).
 *
 * KUnit test-failure hook. drm-core allocators (drm_buddy.c, drm_mm.c) call
 * kunit_fail_current_test() to flag an invariant violation when they are being
 * exercised by an in-tree KUnit test. Outside a KUnit run — which is our case —
 * upstream defines it as a no-op, exactly as CONFIG_KUNIT=n does. Not a fake of
 * real behaviour: there is no current KUnit test to fail. License boundary
 * (../../README.md): API surface.
 */
#ifndef _LINUXKPI_KUNIT_TEST_BUG_H
#define _LINUXKPI_KUNIT_TEST_BUG_H

#define kunit_fail_current_test(fmt, ...) do { } while (0)

static inline void *kunit_get_current_test(void) { return (void *)0; }

#endif /* _LINUXKPI_KUNIT_TEST_BUG_H */
