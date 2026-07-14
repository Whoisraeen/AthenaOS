/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/pci.h> shim (MPL-2.0, original work).
 *
 * The PCI device model — amdgpu IS a PCI device: it reads BARs, config space,
 * MSI/MSI-X, and bus-mastering through this. CRITICAL: this MUST exist as a shim
 * so `#include <linux/pci.h>` resolves HERE and not to the host's
 * /usr/include/linux/pci.h (the host header dragged in host kernel bitops, which
 * is what made `test_bit` conflict). `struct pci_dev` embeds `struct device` by
 * value and carries the fields amdgpu reads by name (vendor, device, revision,
 * subsystem ids, bus, devfn, the BAR resources, irq). The config/BAR/enable/MSI ops are backed
 * by raeen_linuxkpi's PCI facade (P2) at M4 — never faked (a config read that
 * lied would mis-program the GPU; SCOPE.md rule 9). License boundary: API surface.
 */
#ifndef _LINUXKPI_LINUX_PCI_H
#define _LINUXKPI_LINUX_PCI_H

#include <linux/types.h>
#include <linux/device.h>
#include <linux/ioport.h>

struct pci_dev;

#define PCI_BRIDGE_RESOURCE_NUM 4
struct pci_bus {
	struct pci_bus  *parent;
	struct pci_dev  *self;          /* bridge device, or NULL for root */
	unsigned char    number;
	struct resource *resource[PCI_BRIDGE_RESOURCE_NUM];
};
#define pci_bus_for_each_resource(bus, res, i) \
	for ((i) = 0; (i) < PCI_BRIDGE_RESOURCE_NUM && ((res) = (bus)->resource[i], 1); (i)++)

typedef enum {
	pci_channel_io_normal = 1,
	pci_channel_io_frozen,
	pci_channel_io_perm_failure,
} pci_channel_state_t;
typedef enum {
	PCI_ERS_RESULT_NONE = 1,
	PCI_ERS_RESULT_CAN_RECOVER,
	PCI_ERS_RESULT_NEED_RESET,
	PCI_ERS_RESULT_DISCONNECT,
	PCI_ERS_RESULT_RECOVERED,
} pci_ers_result_t;

enum pci_bus_speed {
	PCIE_SPEED_2_5GT = 0x14,
	PCIE_SPEED_5_0GT = 0x15,
	PCIE_SPEED_8_0GT = 0x16,
	PCIE_SPEED_16_0GT = 0x17,
	PCIE_SPEED_32_0GT = 0x18,
	PCIE_SPEED_64_0GT = 0x19,
	PCI_SPEED_UNKNOWN = 0xff,
};
enum pcie_link_width {
	PCIE_LNK_X1 = 1, PCIE_LNK_X2 = 2, PCIE_LNK_X4 = 4, PCIE_LNK_X8 = 8,
	PCIE_LNK_X12 = 12, PCIE_LNK_X16 = 16, PCIE_LNK_X32 = 32,
	PCIE_LNK_WIDTH_UNKNOWN = 0xff,
};
#define PCI_BASE_CLASS_DISPLAY  0x03
#define PCI_CLASS_DISPLAY_VGA   0x0300
#define PCI_CLASS_DISPLAY_OTHER 0x0380
#define PCI_EXP_TYPE_ENDPOINT   0x0
#define PCI_EXP_TYPE_ROOT_PORT  0x4
#define PCI_EXP_TYPE_UPSTREAM   0x5
#define PCI_EXP_TYPE_DOWNSTREAM 0x6
#define PCI_EXP_DEVCAP2         0x24
#define PCI_EXP_DEVCAP2_ATOMIC_COMP32  0x00000080
#define PCI_EXP_DEVCAP2_ATOMIC_COMP64  0x00000100
#define PCI_EXP_DEVCAP2_ATOMIC_COMP128 0x00000200
#define PCI_EXP_LNKCAP          0x0c
#define PCI_EXP_LNKCAP2         0x2c
#define PCI_EXP_LNKCTL2         0x30

typedef enum {
	PCI_D0 = 0,
	PCI_D1,
	PCI_D2,
	PCI_D3hot,
	PCI_D3cold,
	PCI_UNKNOWN,
	PCI_POWER_ERROR = -1,
} pci_power_t;

struct pci_device_id {
	u32 vendor, device;
	u32 subvendor, subdevice;
	u32 class, class_mask;
	unsigned long driver_data;
};

struct pci_dev {
	struct pci_bus *bus;
	struct device   dev;          /* embedded by value */
	unsigned short  vendor;
	unsigned short  device;
	unsigned short  subsystem_vendor;
	unsigned short  subsystem_device;
	unsigned int    class;
	unsigned char   revision;
	unsigned int    devfn;
	int             irq;
	struct resource resource[7];  /* 6 BARs + the expansion ROM at PCI_ROM_RESOURCE */
	unsigned int    is_busmaster : 1;
	unsigned int    msi_enabled : 1;
	unsigned int    msix_enabled : 1;
	u8              msix_cap;      /* MSI-X capability offset (0 = none) */
	u8              msi_cap;
	int             current_state;
	void           *rom;          /* shadow copy of the option ROM (amdgpu_get_bios) */
	size_t          romlen;
};

/* expansion-ROM BAR index (resource[PCI_ROM_RESOURCE]) */
#define PCI_ROM_RESOURCE 6

/* MSI-X message-control register bits (in config space at msix_cap + PCI_MSIX_FLAGS) */
#define PCI_MSIX_FLAGS          2
#define PCI_MSIX_FLAGS_QSIZE    0x07FF
#define PCI_MSIX_FLAGS_ENABLE   0x8000
#define PCI_MSIX_FLAGS_MASKALL  0x4000

/* pci_alloc_irq_vectors() type mask */
#define PCI_IRQ_INTX        (1 << 0)
#define PCI_IRQ_MSI         (1 << 1)
#define PCI_IRQ_MSIX        (1 << 2)
#define PCI_IRQ_ALL_TYPES   (PCI_IRQ_INTX | PCI_IRQ_MSI | PCI_IRQ_MSIX)

struct pci_driver {
	const char *name;
	const struct pci_device_id *id_table;
	int  (*probe)(struct pci_dev *dev, const struct pci_device_id *id);
	void (*remove)(struct pci_dev *dev);
};

/* PCI config-space register offsets + bits (kernel <uapi/linux/pci_regs.h>). */
#define PCI_VENDOR_ID        0x00
#define PCI_DEVICE_ID        0x02
#define PCI_COMMAND          0x04
#define PCI_COMMAND_IO       0x1
#define PCI_COMMAND_MEMORY   0x2
#define PCI_COMMAND_MASTER   0x4
#define PCI_STATUS           0x06
#define PCI_REVISION_ID      0x08
#define PCI_CLASS_DEVICE     0x0a
#define PCI_CACHE_LINE_SIZE  0x0c
#define PCI_BASE_ADDRESS_0   0x10
#define PCI_SUBSYSTEM_VENDOR_ID 0x2c
#define PCI_SUBSYSTEM_ID     0x2e
#define PCI_CAPABILITY_LIST  0x34

#define to_pci_dev(n) container_of(n, struct pci_dev, dev)
#define PCI_DEVFN(slot, func) ((((slot) & 0x1f) << 3) | ((func) & 0x07))
#define PCI_VENDOR_ID_ATI    0x1002
#define PCI_VENDOR_ID_AMD    0x1022
#define PCI_VENDOR_ID_DELL   0x1028
#define PCI_VENDOR_ID_INTEL  0x8086
#define PCI_EXT_CAP_ID_VNDR  0x0b
#define PCI_EXT_CAP_ID_ERR   0x01
#define PCI_EXT_CAP_ID_ATS   0x0f
#define PCI_CAP_ID_EXP       0x10
#define PCI_EXP_DEVCTL       0x08
#define PCI_EXP_LNKCTL       0x10

/* BAR accessors (pure). */
#define pci_resource_start(dev, bar) ((dev)->resource[(bar)].start)
#define pci_resource_end(dev, bar)   ((dev)->resource[(bar)].end)
#define pci_resource_len(dev, bar) \
	((dev)->resource[(bar)].end ? (dev)->resource[(bar)].end - (dev)->resource[(bar)].start + 1 : 0)
#define pci_resource_flags(dev, bar) ((dev)->resource[(bar)].flags)
static inline void *pci_get_drvdata(struct pci_dev *pdev) { return dev_get_drvdata(&pdev->dev); }
static inline void  pci_set_drvdata(struct pci_dev *pdev, void *data) { dev_set_drvdata(&pdev->dev, data); }

/* config space + enable + BAR map + MSI — backed by raeen_linuxkpi (M4) */
int  pci_read_config_byte(struct pci_dev *dev, int where, u8 *val);
int  pci_read_config_word(struct pci_dev *dev, int where, u16 *val);
int  pci_read_config_dword(struct pci_dev *dev, int where, u32 *val);
int  pci_write_config_byte(struct pci_dev *dev, int where, u8 val);
int  pci_write_config_word(struct pci_dev *dev, int where, u16 val);
int  pci_write_config_dword(struct pci_dev *dev, int where, u32 val);
int  pci_enable_device(struct pci_dev *dev);
void pci_disable_device(struct pci_dev *dev);
void pci_set_master(struct pci_dev *dev);
int  pci_request_regions(struct pci_dev *dev, const char *name);
void pci_release_regions(struct pci_dev *dev);
void __iomem *pci_iomap(struct pci_dev *dev, int bar, unsigned long maxlen);
void pci_iounmap(struct pci_dev *dev, void __iomem *addr);
int  pci_enable_msi(struct pci_dev *dev);
int  pci_alloc_irq_vectors(struct pci_dev *dev, unsigned int min, unsigned int max, unsigned int flags);
void pci_free_irq_vectors(struct pci_dev *dev);
int  pci_find_capability(struct pci_dev *dev, int cap);
int  pcie_capability_read_word(struct pci_dev *dev, int pos, u16 *val);

/* config-read all-ones sentinel test (a dead/removed device reads ~0). Macro so
 * it folds at the call site rather than becoming an undefined symbol. */
#define PCI_POSSIBLE_ERROR(val) ((val) == (__typeof__(val))~0)

#endif /* _LINUXKPI_LINUX_PCI_H */
