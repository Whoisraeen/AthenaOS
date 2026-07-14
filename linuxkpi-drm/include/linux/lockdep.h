/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/lockdep.h> shim (MPL-2.0, original work).
 *
 * Runtime lock-dependency validator — a DEBUG facility (CONFIG_LOCKDEP). We build
 * with it OFF (the upstream =n posture, a legitimate config), so the map type is
 * empty and every assert/annotation compiles away. This does not weaken the locks
 * themselves (those are real in spinlock.h/mutex.h) — it only drops the deadlock
 * *checker*. License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_LOCKDEP_H
#define _LINUXKPI_LINUX_LOCKDEP_H

#include <linux/types.h>

struct lockdep_map { };
struct lock_class_key { };

#define lockdep_init_map(map, name, key, sub)   do { } while (0)
#define lockdep_set_class(lock, key)            do { } while (0)
#define lockdep_register_key(key)               do { } while (0)
#define lockdep_unregister_key(key)             do { } while (0)
#define lockdep_assert_held(l)                  do { } while (0)
#define lockdep_assert_held_once(l)             do { } while (0)
#define lockdep_assert_not_held(l)              do { } while (0)
#define lockdep_assert_none_held_once()         do { } while (0)
#define lockdep_is_held(l)                      (1)
#define lock_acquire(map, sc, tr, rd, ch, ne, ip) do { } while (0)
#define lock_release(map, ip)                   do { } while (0)
#define might_lock(lock)                        do { } while (0)
#define might_lock_read(lock)                   do { } while (0)
#define lockdep_set_subclass(lock, sub)         do { } while (0)

#endif /* _LINUXKPI_LINUX_LOCKDEP_H */
