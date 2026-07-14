/* C-link smoke test for ath_linuxkpi.
 *
 * A stand-in "driver" translation unit that calls a broad slice of the LinuxKPI
 * C surface. Its job is purely to LINK: if this object resolves cleanly against
 * libath_linuxkpi.a (the `--features clib --crate-type staticlib` build), then
 * a real Linux .ko — which references the same exported symbols — links the same
 * way. This is the gating proof that the shim is genuinely usable from C, not
 * just self-consistent Rust. Run via scripts/linuxkpi-clink-test.sh.
 *
 * It is NOT meant to run (no hardware, no host syscalls) — only to link.
 */
#include <stddef.h>
#include <stdint.h>

/* memory */
extern void *kmalloc(size_t, unsigned);
extern void *kzalloc(size_t, unsigned);
extern void  kfree(void *);
extern void *__kmalloc(size_t, unsigned);
extern void *kvmalloc_node(size_t, unsigned, int);
/* formatting */
extern int snprintf(char *, size_t, const char *, ...);
extern int scnprintf(char *, size_t, const char *, ...);
extern int _printk(const char *, ...);
/* string */
extern void *memset(void *, int, size_t);
extern size_t strlen(const char *);
/* bitops + atomics */
extern unsigned long _find_first_bit(const unsigned long *, unsigned long);
extern void __bitmap_set(unsigned long *, unsigned, int);
extern void atomic_set(int *, int);
extern int  atomic_read(const int *);
/* ids / refcount */
extern int  ida_alloc(void *, unsigned);
extern int  idr_alloc(void *, void *, int, int, unsigned);
extern void kref_init(int *);
extern int  kref_put(int *, void (*)(int *));
/* kfifo */
extern int  __kfifo_alloc(void *, unsigned, size_t, unsigned);
/* parse */
extern int  kstrtoint(const char *, unsigned, int *);
/* time */
extern uint64_t ktime_get(void);
/* pci */
extern void pci_set_master(uint64_t);
extern int  pci_find_ext_capability(uint64_t, uint16_t);
/* dma-buf framework */
extern uint64_t dma_fence_context_alloc(uint64_t);
extern void dma_fence_init(void *, const void *, void *, uint64_t, uint64_t);
extern int  dma_fence_signal(void *);
extern long dma_fence_wait_timeout(void *, int, long);
extern int  dma_resv_reserve_fences(void *, unsigned);
extern void dma_resv_add_fence(void *, void *, unsigned);
extern void *dma_buf_map_attachment(void *, int);
/* scatterlist */
extern void sg_init_one(void *, void *, unsigned);

void probe_main(void) {
    char buf[64];
    void *p = kmalloc(128, 0);
    kfree(p);
    p = kzalloc(16, 0); kfree(p);
    p = __kmalloc(64, 0); kfree(p);
    p = kvmalloc_node(32, 0, 0); kfree(p);
    snprintf(buf, sizeof buf, "ver=%d", 1);
    scnprintf(buf, sizeof buf, "%s", "x");
    _printk("probe %d\n", 7);
    memset(buf, 0, sizeof buf); (void)strlen(buf);
    unsigned long bm = 0; __bitmap_set(&bm, 0, 1); (void)_find_first_bit(&bm, 64);
    int a = 0; atomic_set(&a, 3); (void)atomic_read(&a);
    long ida = 0; (void)ida_alloc(&ida, 0);
    long idr = 0; (void)idr_alloc(&idr, buf, 0, 0, 0);
    int kr = 0; kref_init(&kr); kref_put(&kr, 0);
    long fifo[8]; (void)__kfifo_alloc(fifo, 8, 4, 0);
    int iv; (void)kstrtoint("42", 10, &iv);
    (void)ktime_get();
    pci_set_master(0); (void)pci_find_ext_capability(0, 1);
    (void)dma_fence_context_alloc(1);
    char fence[64]; dma_fence_init(fence, 0, 0, 0, 0);
    dma_fence_signal(fence); (void)dma_fence_wait_timeout(fence, 0, 1);
    char resv[48]; (void)dma_resv_reserve_fences(resv, 1); dma_resv_add_fence(resv, fence, 1);
    char sg[32]; sg_init_one(sg, buf, 16);
    (void)dma_buf_map_attachment(0, 0);
}
