/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/console.h> shim (MPL-2.0, original work).
 *
 * Kernel console registration + locking. amdgpu takes console_lock() around the
 * fbdev/mode handoff to serialise against the boot console. In the AthenaOS daemon
 * model there is no kernel printk console to fight over (the daemon logs via the
 * M4 log facade), so the lock calls are honest no-ops and (un)register is backed
 * by ath_linuxkpi if ever needed. License boundary (../../README.md): surface.
 */
#ifndef _LINUXKPI_LINUX_CONSOLE_H
#define _LINUXKPI_LINUX_CONSOLE_H

#include <linux/types.h>

struct console;

#define console_lock()         do { } while (0)
#define console_unlock()       do { } while (0)
#define console_trylock()      (1)
#define console_suspend_all()  do { } while (0)
#define console_resume_all()   do { } while (0)

void register_console(struct console *con);
int  unregister_console(struct console *con);

#endif /* _LINUXKPI_LINUX_CONSOLE_H */
