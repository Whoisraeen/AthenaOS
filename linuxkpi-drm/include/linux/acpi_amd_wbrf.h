/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/acpi_amd_wbrf.h> shim (MPL-2.0, original work).
 *
 * AMD WBRF (Wifi Band RF Filtering) — an ACPI interface for the GPU/SMU to tell
 * the Wi-Fi stack which frequency bands to avoid (RFI mitigation). Out of the MES
 * bring-up subset (SCOPE.md). The SMU header includes it for the notifier/range
 * types; the register/notify ops are backed by ath_linuxkpi at M4 if WBRF is
 * ever in scope. License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_ACPI_AMD_WBRF_H
#define _LINUXKPI_LINUX_ACPI_AMD_WBRF_H

#include <linux/types.h>

struct device;
struct notifier_block;

#define MAX_NUM_OF_WBRF_RANGES 11

struct freq_band_range {
	u64 start;
	u64 end;
};

struct wbrf_ranges_in_out {
	u64 num_of_ranges;
	struct freq_band_range band_list[MAX_NUM_OF_WBRF_RANGES];
};

enum wbrf_notifier_actions {
	WBRF_CHANGED,
};

/* WBRF ops — backed by ath_linuxkpi (M4) if brought into scope */
bool wbrf_supported_producer(struct device *dev);
int  wbrf_register_notifier(struct notifier_block *nb);
int  wbrf_unregister_notifier(struct notifier_block *nb);
int  wbrf_add_exclusion(struct device *dev, struct wbrf_ranges_in_out *in);
int  wbrf_remove_exclusion(struct device *dev, struct wbrf_ranges_in_out *in);
int  wbrf_retrieve_freq_band(struct device *dev, struct wbrf_ranges_in_out *out);

#endif /* _LINUXKPI_LINUX_ACPI_AMD_WBRF_H */
