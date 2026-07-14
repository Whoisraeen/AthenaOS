/* SPDX-License-Identifier: MPL-2.0 */
/*
 * rae_implicit_fence.h — force-included (gcc -include) into every vendored TU
 * by m4-link.sh. MPL-2.0, original work (API prototypes only).
 *
 * WHY (2026-07-08, found off-target by tools/amdgpu_hostrun): the curated
 * include tree does not replicate the kernel's transitive include chains, so
 * some TUs call functions with NO prototype in scope. The build tolerates that
 * (GCC_COMPAT -Wno-implicit-function-declaration) because off-path link-time
 * stubbing depends on it — but gcc then assumes `int f()`, and for POINTER- or
 * 64-BIT-returning functions the CALLER truncates rax to eax and sign-extends
 * (`call ioremap; cltq`). That corrupted `adev->rmmio` in amdgpu_device_init
 * (0x00007ffff7ee0000 became 0xfffffffff7ee0000) → the first RREG32 dereferenced
 * a wild pointer: clean SIGSEGV on the host runner, and the prime suspect class
 * for the M1 iron wedge (docs/gpu-oracle/M1-VERDICT-20260706.md).
 *
 * THE RULE: every implicitly-declared function that returns a pointer or a
 * 64-bit integer MUST have an exact-signature prototype here (or in a curated
 * header this one includes). int-returning implicits stay tolerated — they are
 * ABI-safe on x86-64 SysV. Functions defined `static inline` in a curated
 * header (the kmap family in <linux/highmem.h>) must NOT be prototyped here;
 * m4-link.sh force-includes that header for the TTM TUs instead.
 *
 * Regenerate the audit:
 *   sed 's|2>/dev/null|2>>/tmp/m4-warn.log|g;
 *        s|-Wno-implicit-function-declaration|-Wno-error=implicit-function-declaration|' \
 *     linuxkpi-drm/m4-link.sh > linuxkpi-drm/m4-link-warn.tmp.sh
 *   bash linuxkpi-drm/m4-link-warn.tmp.sh; rm linuxkpi-drm/m4-link-warn.tmp.sh
 *   grep -oP "implicit declaration of function .\K[A-Za-z0-9_]+" /tmp/m4-warn.log | sort -u
 */
#ifndef _RAE_IMPLICIT_FENCE_H
#define _RAE_IMPLICIT_FENCE_H

/* The io family (ioremap/readl/memremap/...) — the confirmed live truncation:
 * amdgpu_device.c stored a cltq'd ioremap() return into adev->rmmio. */
#include <linux/io.h>

/* Forward decls only — this header is prepended before everything else. */
struct page;
struct pci_dev;
struct device;
struct kobject;
struct kset;
struct dma_fence;
struct dma_resv;
struct ww_acquire_ctx;
struct irq_domain;
struct workqueue_struct;
struct pci_saved_state;
struct radix_tree_iter;

/* ── mm: pointer returns ── */
struct page *vmalloc_to_page(const void *addr);
struct page *hmm_pfn_to_page(unsigned long hmm_pfn);
struct page *alloc_pages_node(int nid, unsigned int gfp_mask, unsigned int order);
unsigned long *bitmap_zalloc(unsigned int nbits, unsigned int flags);
void *memdup_user(const void *src, size_t len);
void *memdup_user_nul(const void *src, size_t len);
extern unsigned int overflowuid;
void *memdup_array_user(const void *src, size_t n, size_t size);
void *vmemdup_array_user(const void *src, size_t n, size_t size);
void *radix_tree_iter_delete(void *root, struct radix_tree_iter *iter, void **slot);

/* ── atomic_long_*: 64-bit long returns (TTM page accounting, fence counters).
 * Backed by REAL SeqCst ops in raeen_linuxkpi (drm_bringup.rs). ── */
long atomic_long_read(const long *v);
void atomic_long_set(long *v, long i);
void atomic_long_add(long i, long *v);
void atomic_long_sub(long i, long *v);
long atomic_long_cmpxchg(long *v, long old_val, long new_val);

/* ── bitmap multi-bit accessors: unsigned-long returns (amdgpu_utils.h caps) ── */
unsigned long bitmap_read(const unsigned long *map, unsigned long start, unsigned long nbits);
void bitmap_write(unsigned long *map, unsigned long value, unsigned long start, unsigned long nbits);

/* ── dma_fence / dma_resv: pointer + u64 returns ── */
struct dma_fence *dma_fence_get_stub(void);
unsigned long long dma_fence_context_alloc(unsigned int num);
struct ww_acquire_ctx *dma_resv_locking_ctx(struct dma_resv *obj);

/* ── device model: pointer returns ── */
struct device *kobj_to_dev(struct kobject *kobj);
const char *kobject_name(const struct kobject *kobj);
struct kset *to_kset(struct kobject *kobj);
const char *dev_driver_string(const struct device *dev);
struct workqueue_struct *create_singlethread_workqueue(const char *name);
struct irq_domain *irq_domain_create_linear(void *fwnode, unsigned int size,
					    const void *ops, void *host_data);

/* ── PCI: pointer returns ── */
struct pci_dev *pci_get_domain_bus_and_slot(int domain, unsigned int bus, unsigned int devfn);
struct pci_dev *pci_get_base_class(unsigned char base_class, struct pci_dev *from);
struct pci_dev *pcie_find_root_port(struct pci_dev *dev);
struct pci_dev *pci_upstream_bridge(struct pci_dev *dev);
void *pci_map_rom(struct pci_dev *pdev, size_t *size);
struct pci_saved_state *pci_store_saved_state(struct pci_dev *dev);
const char *pci_name(const struct pci_dev *pdev);

/* ── strings: pointer returns ── */
char *strnstr(const char *s1, const char *s2, size_t len);

/* ── time / pm / uaccess: 64-bit integer returns ── */
unsigned long nsecs_to_jiffies(unsigned long long n);
unsigned long long pm_runtime_autosuspend_expiration(struct device *dev);
unsigned long copy_from_user(void *to, const void *from, unsigned long n);
unsigned long copy_to_user(void *to, const void *from, unsigned long n);

#endif /* _RAE_IMPLICIT_FENCE_H */
