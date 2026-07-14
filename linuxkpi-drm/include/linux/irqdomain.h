/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/irqdomain.h> shim (MPL-2.0, original work).
 *
 * IRQ-domain (interrupt-controller virtual->hw mapping). amdgpu's IH ring wraps
 * its source decoding behind a domain. In the AthenaOS userspace-daemon model a
 * device IRQ is delivered as IPC (raeen_linuxkpi's IRQ-doorbell facade, P2), not
 * a kernel irq line, so the mapping calls are backed by that facade at M4. This
 * is the type/decl surface the IH header needs. License boundary: API surface.
 */
#ifndef _LINUXKPI_LINUX_IRQDOMAIN_H
#define _LINUXKPI_LINUX_IRQDOMAIN_H

#include <linux/types.h>
/* amdgpu_irq.h includes us first, then amdgpu_ih.h — both embed spinlock_t /
 * wait_queue_head_t / work_struct without including their headers (kernel
 * transitivity). Provide the trio here so those amd headers typecheck. */
#include <linux/spinlock.h>
#include <linux/wait.h>
#include <linux/workqueue.h>

struct irq_domain;
struct irq_domain_ops;
struct irq_fwspec;
struct fwnode_handle;

typedef unsigned long irq_hw_number_t;

struct irq_domain_ops {
	int  (*map)(struct irq_domain *d, unsigned int virq, irq_hw_number_t hw);
	void (*unmap)(struct irq_domain *d, unsigned int virq);
	int  (*alloc)(struct irq_domain *d, unsigned int virq, unsigned int nr_irqs, void *arg);
	void (*free)(struct irq_domain *d, unsigned int virq, unsigned int nr_irqs);
};

/* backed by raeen_linuxkpi's IRQ facade (M4) */
struct irq_domain *irq_domain_add_linear(struct fwnode_handle *fwnode, unsigned int size,
					 const struct irq_domain_ops *ops, void *host_data);
void          irq_domain_remove(struct irq_domain *domain);
unsigned int  irq_create_mapping(struct irq_domain *domain, irq_hw_number_t hwirq);
unsigned int  irq_find_mapping(struct irq_domain *domain, irq_hw_number_t hwirq);
void          irq_dispose_mapping(unsigned int virq);

#endif /* _LINUXKPI_LINUX_IRQDOMAIN_H */
