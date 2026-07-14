/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/module.h> shim (MPL). amdgpu uses this mostly for the MODULE_*()
 * registration macros (firmware manifest, params, license). On AthenaOS the
 * driver is a userspace daemon, not a loadable module, so these macros are
 * compile-time no-ops — but MODULE_FIRMWARE() strings are still meaningful (the
 * blob manifest), so we keep them as discardable annotations.
 */
#ifndef _LINUXKPI_LINUX_MODULE_H
#define _LINUXKPI_LINUX_MODULE_H

#include <linux/types.h>
/* upstream module.h transitively pulls the core allocation/overflow machinery;
 * drm-core allocators (drm_buddy.c) reach range_overflows() this way. */
#include <linux/overflow.h>

struct module;
#define THIS_MODULE ((struct module *)0)
#ifndef KBUILD_MODNAME
#define KBUILD_MODNAME "ath_amdgpu"
#endif

/* registration macros — no-ops in the userspace-daemon model */
#define MODULE_FIRMWARE(_name)
#define MODULE_AUTHOR(_s)
#define MODULE_DESCRIPTION(_s)
#define MODULE_LICENSE(_s)
#define MODULE_VERSION(_s)
#define MODULE_DEVICE_TABLE(_type, _tbl)
#define MODULE_PARM_DESC(_p, _s)
#define MODULE_ALIAS(_s)
#define MODULE_IMPORT_NS(_ns)
#define MODULE_SOFTDEP(_s)
#define MODULE_INFO(_tag, _info)

/* Linux runs each driver/subsystem's module_init() as a boot-time initcall.
 * The bring-up daemon has no module loader, so module_init emits a named extern
 * wrapper (rae_ic_<fn>) around the file-static initcall; bringup_drm.c calls the
 * ones the compiled subset needs (see rae_run_initcalls) before the driver init
 * touches them. drm_buddy's initcall creates the drm_buddy_block slab cache —
 * without it kmem_cache_zalloc(NULL) returns NULL and drm_buddy_init() fails
 * -ENOMEM. A named wrapper avoids fragile linker __start_/__stop_ section
 * encapsulation across the ld -r + final-link boundary. Faithful to the
 * initcall contract (the function still runs exactly once, at bring-up). */
#define module_init(_fn) \
	int rae_ic_##_fn(void) { return (_fn)(); }
#define module_exit(_fn)   /* teardown is not run in the headless daemon */
#define module_param(_n, _t, _p)
#define module_param_named(_n, _v, _t, _p)

#ifndef __init
#define __init
#endif
#ifndef __exit
#define __exit
#endif
#ifndef __must_check
#define __must_check
#endif

#endif /* _LINUXKPI_LINUX_MODULE_H */
