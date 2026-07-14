/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/kmsg_dump.h> shim (MPL-2.0, original work).
 *
 * Kernel-log dumper (capture dmesg on panic/oops). Reached only via the DRM plane
 * type graph; amdgpu registers a dumper to snapshot its ring state on GPU hang.
 * Not present in the userspace-daemon model (register is a no-op returning
 * success-of-nothing) — the daemon's own crash path + /proc/raeen carry that
 * diagnostic instead. Honest "no kmsg dumper", not a fake of a functional path.
 * License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_KMSG_DUMP_H
#define _LINUXKPI_LINUX_KMSG_DUMP_H

#include <linux/types.h>

enum kmsg_dump_reason {
	KMSG_DUMP_UNDEF,
	KMSG_DUMP_PANIC,
	KMSG_DUMP_OOPS,
	KMSG_DUMP_EMERG,
	KMSG_DUMP_SHUTDOWN,
	KMSG_DUMP_MAX,
};

struct kmsg_dumper {
	void (*dump)(struct kmsg_dumper *dumper, enum kmsg_dump_reason reason);
	enum kmsg_dump_reason max_reason;
	bool registered;
};

static inline int kmsg_dump_register(struct kmsg_dumper *dumper) { if (dumper) dumper->registered = false; return 0; }
static inline int kmsg_dump_unregister(struct kmsg_dumper *dumper) { (void)dumper; return 0; }

#endif /* _LINUXKPI_LINUX_KMSG_DUMP_H */
