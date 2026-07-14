/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/irqreturn.h> shim (MPL-2.0, original work).
 *
 * The IRQ-handler return enum (its own header in the kernel; <linux/interrupt.h>
 * includes it). Kept here so both that and direct includers agree on the type.
 * License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_IRQRETURN_H
#define _LINUXKPI_LINUX_IRQRETURN_H

typedef enum irqreturn { IRQ_NONE = 0, IRQ_HANDLED = 1, IRQ_WAKE_THREAD = 2 } irqreturn_t;

#endif /* _LINUXKPI_LINUX_IRQRETURN_H */
