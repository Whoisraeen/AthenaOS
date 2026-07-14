/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/seqlock.h> shim (MPL-2.0, original work).
 *
 * Sequence locks — lockless reads that retry if a writer intervened. drm_vblank
 * keeps its vblank timestamp in a seqcount_latch so the present path can read it
 * without blocking the IRQ writer. Real inline counter logic (the read/retry
 * dance is correct); the embedded spinlock_t comes from the ath_linuxkpi sync
 * facade. License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_SEQLOCK_H
#define _LINUXKPI_LINUX_SEQLOCK_H

#include <linux/types.h>
#include <linux/compiler.h>
#include <linux/spinlock.h>

typedef struct seqcount {
	unsigned sequence;
} seqcount_t;

/* the "latch" variant keeps two copies so a reader never sees a torn update. */
typedef struct seqcount_latch {
	seqcount_t seqcount;
} seqcount_latch_t;

typedef struct {
	seqcount_t  seqcount;
	spinlock_t  lock;
} seqlock_t;

#define seqcount_init(s)       do { (s)->sequence = 0; } while (0)
#define SEQCNT_ZERO(name)      { 0 }
#define seqcount_latch_init(s) do { (s)->seqcount.sequence = 0; } while (0)
#define seqlock_init(s)        do { (s)->seqcount.sequence = 0; spin_lock_init(&(s)->lock); } while (0)

static inline unsigned __read_seqcount_begin(const seqcount_t *s)
{
	unsigned ret;
	do {
		ret = READ_ONCE(s->sequence);
	} while (ret & 1);
	barrier();
	return ret;
}
static inline unsigned read_seqcount_begin(const seqcount_t *s)
{
	return __read_seqcount_begin(s);
}
static inline int read_seqcount_retry(const seqcount_t *s, unsigned start)
{
	barrier();
	return READ_ONCE(s->sequence) != start;
}
static inline void write_seqcount_begin(seqcount_t *s) { s->sequence++; barrier(); }
static inline void write_seqcount_end(seqcount_t *s)   { barrier(); s->sequence++; }

/* latch read/write — the reader picks a copy by the low bit; we model the
 * counter exactly (the dual-copy storage is the caller's two-element array). */
static inline unsigned raw_read_seqcount_latch(const seqcount_latch_t *s)
{
	return READ_ONCE(s->seqcount.sequence);
}
static inline int raw_read_seqcount_latch_retry(const seqcount_latch_t *s, unsigned start)
{
	barrier();
	return READ_ONCE(s->seqcount.sequence) != start;
}
static inline void raw_write_seqcount_latch(seqcount_latch_t *s)
{
	barrier();
	s->seqcount.sequence++;
	barrier();
}

/* seqlock_t (seqcount + spinlock) read side */
static inline unsigned read_seqbegin(const seqlock_t *sl)
{
	return read_seqcount_begin(&sl->seqcount);
}
static inline int read_seqretry(const seqlock_t *sl, unsigned start)
{
	return read_seqcount_retry(&sl->seqcount, start);
}
static inline void write_seqlock(seqlock_t *sl)   { spin_lock(&sl->lock);   write_seqcount_begin(&sl->seqcount); }
static inline void write_sequnlock(seqlock_t *sl) { write_seqcount_end(&sl->seqcount); spin_unlock(&sl->lock); }

#endif /* _LINUXKPI_LINUX_SEQLOCK_H */
