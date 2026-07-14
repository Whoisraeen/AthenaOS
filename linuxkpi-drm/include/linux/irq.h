/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/irq.h> shim (MPL-2.0, original work).
 *
 * The generic-IRQ-layer types (irq_chip / irq_data / flow handlers). amdgpu_irq
 * builds its interrupt-source dispatch on top of the IRQ domain (irqdomain.h) and
 * touches a few of these types. The real interrupt delivery is the AthenaOS kernel's
 * job; the daemon receives demuxed IRQ events over its IRQ-wait syscall, so the
 * chip/flow ops here are layout-only, backed by ath_linuxkpi at M4. License
 * boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_IRQ_H
#define _LINUXKPI_LINUX_IRQ_H

#include <linux/types.h>
#include <linux/irqreturn.h>
#include <linux/irqdomain.h>

struct irq_data {
	u32                  irq;
	unsigned long        hwirq;
	struct irq_chip     *chip;
	void                *chip_data;
	struct irq_domain   *domain;
};

struct irq_chip {
	const char *name;
	void (*irq_enable)(struct irq_data *data);
	void (*irq_disable)(struct irq_data *data);
	void (*irq_ack)(struct irq_data *data);
	void (*irq_mask)(struct irq_data *data);
	void (*irq_unmask)(struct irq_data *data);
	int  (*irq_set_affinity)(struct irq_data *data, const struct cpumask *dest, bool force);
};

typedef void (*irq_flow_handler_t)(struct irq_data *data);

/* flow handlers + chip wiring — backed by ath_linuxkpi (M4) */
void handle_level_irq(struct irq_data *data);
void handle_edge_irq(struct irq_data *data);
void handle_simple_irq(struct irq_data *data);
void handle_fasteoi_irq(struct irq_data *data);
struct irq_data *irq_get_irq_data(unsigned int irq);
int  generic_handle_irq(unsigned int irq);
void irq_set_chip_and_handler(unsigned int irq, const struct irq_chip *chip, irq_flow_handler_t handle);
void irq_set_chip_data(unsigned int irq, void *data);
void *irq_get_chip_data(unsigned int irq);

#endif /* _LINUXKPI_LINUX_IRQ_H */
