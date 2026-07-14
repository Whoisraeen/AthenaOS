/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/acpi.h> shim (MPL-2.0, original work).
 *
 * ACPI handle/table surface. amdgpu reaches it for the VFCT path (a VBIOS image
 * embedded in an ACPI table — one VBIOS fallback) and the ATPX/ATCS hybrid-gfx
 * methods (out of the bring-up subset, AthGuard owns GPU arbitration). The
 * AthenaOS ACPI namespace is owned by the kernel; the userspace bring-up daemon
 * gets tables via the host, so acpi_get_table is backed by ath_linuxkpi at M4
 * (reports "absent" until wired — the PCI-ROM VBIOS path is primary). License
 * boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_ACPI_H
#define _LINUXKPI_LINUX_ACPI_H

#include <linux/types.h>
#include <linux/device.h>

typedef void *acpi_handle;
typedef u32   acpi_status;
typedef u64   acpi_size;

#define AE_OK         0x0000
#define AE_NOT_FOUND  0x0005
#define ACPI_SUCCESS(s)  ((s) == AE_OK)
#define ACPI_FAILURE(s)  ((s) != AE_OK)

struct acpi_table_header {
	char     signature[4];
	u32      length;
	u8       revision;
	u8       checksum;
	char     oem_id[6];
	char     oem_table_id[8];
	u32      oem_revision;
	char     asl_compiler_id[4];
	u32      asl_compiler_revision;
};

struct acpi_buffer {
	acpi_size length;
	void     *pointer;
};

/* Full ACPI object surface — amdgpu_bios.c's ATRM path (also #ifdef CONFIG_ACPI,
 * enabled alongside the VFCT VBIOS path) declares `union acpi_object` by value and
 * uses ACPI_ALLOCATE_BUFFER / ACPI_TYPE_INTEGER. ATRM itself no-ops here
 * (acpi_evaluate_object is the AE_NOT_FOUND stub), but the type must be complete
 * for the file to compile. Layout mirrors <acpi/actypes.h>. */
typedef u32 acpi_object_type;
#define ACPI_TYPE_INTEGER 0x01
#define ACPI_TYPE_STRING  0x02
#define ACPI_TYPE_BUFFER  0x03
#define ACPI_TYPE_PACKAGE 0x04
#define ACPI_ALLOCATE_BUFFER ((acpi_size)-1)

union acpi_object {
	acpi_object_type type; /* common first field */
	struct {
		acpi_object_type type;
		u64 value;
	} integer;
	struct {
		acpi_object_type type;
		u32 length;
		u8 *pointer;
	} buffer;
	struct {
		acpi_object_type type;
		u32 length;
		char *pointer;
	} string;
};

struct acpi_object_list {
	u32 count;
	union acpi_object *pointer;
};

/* device <-> ACPI handle */
#define ACPI_HANDLE(dev) ((acpi_handle)NULL)
acpi_handle acpi_device_handle(struct device *dev);

/* table + method access — backed by ath_linuxkpi (M4) */
acpi_status acpi_get_table(char *signature, u32 instance, struct acpi_table_header **out);
void        acpi_put_table(struct acpi_table_header *table);
acpi_status acpi_evaluate_object(acpi_handle handle, char *pathname,
				 struct acpi_object_list *params, struct acpi_buffer *result);

static inline bool acpi_disabled_stub(void) { return true; }

#endif /* _LINUXKPI_LINUX_ACPI_H */
