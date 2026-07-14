/* SPDX-License-Identifier: MPL-2.0 */
/* AthenaOS does not enable x86 SME/SEV page encryption for userspace driver
 * mappings. The page-protection transform is therefore the identity supplied
 * by the curated pgtable shim. */
#ifndef _LINUXKPI_LINUX_MEM_ENCRYPT_H
#define _LINUXKPI_LINUX_MEM_ENCRYPT_H

#include <linux/pgtable.h>

#endif
