/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/seq_file.h> shim (MPL-2.0, original work).
 *
 * Sequential-file output (debugfs/procfs dumps). amdgpu's ring/fence/ib info
 * handlers write through seq_printf. Debug-output path, out of the MES bring-up
 * subset; the writers are backed by raeen_linuxkpi at M4 (the daemon's /proc/raeen
 * surface). License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_SEQ_FILE_H
#define _LINUXKPI_LINUX_SEQ_FILE_H

#include <linux/types.h>
#include <linux/compiler.h>

struct seq_file {
	char  *buf;
	size_t size;
	size_t count;
	void  *private;
};

struct file;

__printf(2, 3) void seq_printf(struct seq_file *m, const char *fmt, ...);
void seq_puts(struct seq_file *m, const char *s);
void seq_putc(struct seq_file *m, char c);
void seq_write(struct seq_file *m, const void *data, size_t len);

#endif /* _LINUXKPI_LINUX_SEQ_FILE_H */
