/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <asm/processor.h> shim (MPL-2.0, original work).
 *
 * The x86 CPU-identity record. amdgpu/TTM read `boot_cpu_data` for the CPU family
 * (feature gating) and cache-line/clflush size (BO flush granularity). Athena is a
 * Ryzen 5 7640HS — Family 19h (Zen 4), 64-byte cache lines — so the exported
 * `boot_cpu_data` (ath_linuxkpi) carries those real values, gating the correct
 * AMD paths. License boundary (../../README.md): API surface + ABI layout.
 */
#ifndef _LINUXKPI_ASM_PROCESSOR_H
#define _LINUXKPI_ASM_PROCESSOR_H

#include <linux/types.h>

/* Layout must match the ath_linuxkpi export (drm_bringup.rs::boot_cpu_data). */
struct cpuinfo_x86 {
	__u8         x86;               /* CPU family */
	__u8         x86_vendor;
	__u8         x86_model;
	__u8         x86_stepping;
	int          x86_clflush_size;  /* cache-line / flush granularity */
	int          x86_cache_alignment;
	__u32        x86_capability[24];
	char         x86_model_id[64];
	unsigned int x86_max_cores;
	__u64        _reserved[16];
};

extern struct cpuinfo_x86 boot_cpu_data;
#define cpu_data(cpu) boot_cpu_data

#endif /* _LINUXKPI_ASM_PROCESSOR_H */
