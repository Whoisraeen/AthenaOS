/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/kfifo.h> shim (MPL-2.0, original work).
 *
 * Byte/record FIFO. amdgpu RAS logs ECC error records through one. This is an
 * ORIGINAL implementation of the kfifo API CONTRACT (element-size based) — NOT a
 * transcription of the kernel's union/macro machinery. A typed handle pairs the
 * `struct __kfifo` base with a typed buffer pointer so the put/get macros infer
 * the element size; the actual ring ops are backed by raeen_linuxkpi's kfifo
 * (one of its 488 exports) at M4 — never faked to silently drop records
 * (SCOPE.md rule 9). License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_KFIFO_H
#define _LINUXKPI_LINUX_KFIFO_H

#include <linux/types.h>

struct __kfifo {
	unsigned int in;
	unsigned int out;
	unsigned int mask;   /* size-1 (size is a power of two) */
	unsigned int esize;  /* bytes per element */
	void        *data;
};

/* the bare `struct kfifo` (byte fifo) embedded by value in amd RAS state — a
 * complete typed handle (element = unsigned char) so it lays out. */
struct kfifo {
	struct __kfifo kfifo;
	unsigned char *buf;
};

/* typed handles: base + a typed buffer for element-size inference. */
#define DECLARE_KFIFO_PTR(fifo, type)      struct { struct __kfifo kfifo; type *buf; } fifo
#define DECLARE_KFIFO(fifo, type, size)    struct { struct __kfifo kfifo; type buf[size]; } fifo
#define STRUCT_KFIFO_PTR(type)             struct { struct __kfifo kfifo; type *buf; }
#define INIT_KFIFO(fifo) \
	do { \
		(fifo).kfifo.in = 0; (fifo).kfifo.out = 0; \
		(fifo).kfifo.esize = sizeof((fifo).buf[0]); \
		(fifo).kfifo.mask = (sizeof((fifo).buf) / sizeof((fifo).buf[0])) - 1; \
		(fifo).kfifo.data = (fifo).buf; \
	} while (0)

/* ring ops on the base — backed by raeen_linuxkpi (M4). */
int          __kfifo_alloc(struct __kfifo *fifo, unsigned int size, size_t esize, gfp_t gfp);
void         __kfifo_free(struct __kfifo *fifo);
unsigned int __kfifo_in(struct __kfifo *fifo, const void *buf, unsigned int len);
unsigned int __kfifo_out(struct __kfifo *fifo, void *buf, unsigned int len);
unsigned int __kfifo_out_peek(struct __kfifo *fifo, void *buf, unsigned int len);

#define kfifo_len(fifo)        (unsigned int)((fifo)->kfifo.in - (fifo)->kfifo.out)
#define kfifo_size(fifo)       ((fifo)->kfifo.mask + 1)
#define kfifo_is_empty(fifo)   ((fifo)->kfifo.in == (fifo)->kfifo.out)
#define kfifo_is_full(fifo)    (kfifo_len(fifo) > (fifo)->kfifo.mask)
#define kfifo_avail(fifo)      (kfifo_size(fifo) - kfifo_len(fifo))
#define kfifo_reset(fifo)      do { (fifo)->kfifo.in = (fifo)->kfifo.out = 0; } while (0)

#define kfifo_alloc(fifo, size, gfp) \
	__kfifo_alloc(&(fifo)->kfifo, (size), sizeof(*(fifo)->buf), (gfp))
#define kfifo_free(fifo) __kfifo_free(&(fifo)->kfifo)

#define kfifo_put(fifo, val) ({ \
	__typeof__(*(fifo)->buf) __kf_v = (val); \
	__kfifo_in(&(fifo)->kfifo, &__kf_v, 1); })
#define kfifo_get(fifo, valptr) \
	__kfifo_out(&(fifo)->kfifo, (valptr), 1)
#define kfifo_peek(fifo, valptr) \
	__kfifo_out_peek(&(fifo)->kfifo, (valptr), 1)
#define kfifo_in(fifo, buf, n)  __kfifo_in(&(fifo)->kfifo, (buf), (n))
#define kfifo_out(fifo, buf, n) __kfifo_out(&(fifo)->kfifo, (buf), (n))

#endif /* _LINUXKPI_LINUX_KFIFO_H */
