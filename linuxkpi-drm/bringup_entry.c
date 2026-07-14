// SPDX-License-Identifier: MPL-2.0
/*
 * bringup_entry.c — the amdgpud daemon's entry into the real amdgpu init (MPL-2.0,
 * original work).
 *
 * The daemon claims the Athena GPU (BDF c4:00.0), wires ath_linuxkpi's device
 * access (lkpi_set_current_device + lkpi_register_bar per BAR), then calls
 * rae_amdgpu_device_init() with the discovered PCI identity + BAR windows. This
 * replicates the minimal setup amdgpu_pci_probe() does before handing off to the
 * real amdgpu_driver_load_kms() -> amdgpu_device_init() — the complete upstream
 * init the Rust reimpl could not fully reproduce (it halts at 0x7654).
 *
 * This is the seam where the compiled+linked+cross-built amdgpu object graph meets
 * live hardware. Struct types come from the compiled-against amdgpu.h; the entry is
 * original.
 */
#include "amdgpu.h"

/* Phoenix (and all modern APUs/dGPUs) init via the on-chip IP-discovery table. */
#ifndef CHIP_IP_DISCOVERY
#define CHIP_IP_DISCOVERY 36
#endif

/* Phoenix is an APU (UMA VRAM carved from system RAM). AMD_IS_APU selects the
 * APU aper_base override (aper_base = mmhub FB offset, aper_size = real VRAM)
 * and skips the discrete-GPU FB-BAR resize in gmc_v11_0_mc_init — matching how
 * real Linux treats 1002:15bf. Without it gmc runs the discrete path and the
 * PSP TMR/aperture environment diverges (SETUP_TMR never completes). */
#ifndef AMD_IS_APU
#define AMD_IS_APU 0x00020000UL
#endif

int amdgpu_driver_load_kms(struct amdgpu_device *adev, unsigned long flags);
extern void rae_diag_netlog_flush(void);
/* bringup_drm.c — initialise the embedded drm_device fields (vma_offset_manager,
 * anon_inode) the skipped drm_dev_init would set; ttm_device_init derefs them. */
void rae_amdgpu_setup_ddev(struct amdgpu_device *adev);
/* bringup_drm.c — run the collected module_init() initcalls (drm_buddy's creates
 * the VRAM allocator's slab cache) before the driver init touches them. */
void rae_run_initcalls(void);
int rae_amdgpu_render_device_init(struct amdgpu_device *adev);

/* The initialized upstream device is daemon-owned and must outlive the init
 * call. Keeping it here gives every later render client one authoritative
 * amdgpu_device instead of rebuilding GPU state in the Rust scaffold. */
static struct amdgpu_device *rae_adev;

static void rae_set_bar(struct pci_dev *pdev, unsigned int i, u64 phys, u64 size)
{
	if (i < 6 && size) {
		pdev->resource[i].start = phys;
		pdev->resource[i].end   = phys + size - 1;
		pdev->resource[i].flags = IORESOURCE_MEM;
	}
}

/*
 * Run the real amdgpu init against the claimed device. The daemon has already
 * called lkpi_set_current_device(handle) + lkpi_register_bar(bar, phys, size), so
 * ath_linuxkpi's ioremap/dma/config route to the live GPU. Returns 0 on success.
 */
int rae_amdgpu_device_init(u16 vendor, u16 device, u8 revision,
			   u8 pci_bus, u8 pci_devfn,       /* claimed BDF   */
			   u64 bar0_phys, u64 bar0_size,   /* VRAM aperture */
			   u64 bar2_phys, u64 bar2_size,   /* doorbell      */
			   u64 bar5_phys, u64 bar5_size)   /* registers     */
{
	struct pci_dev *pdev;
	struct pci_bus *bus;
	struct amdgpu_device *adev;
	unsigned long flags = CHIP_IP_DISCOVERY | AMD_IS_APU;
	int r;

	pr_info("RAE-ENTRY vendor=0x%04x device=0x%04x rev=0x%02x bdf=%02x:%02x bars=%llx/%llx/%llx\n",
		vendor, device, revision, pci_bus, pci_devfn,
		bar0_phys, bar2_phys, bar5_phys);
	rae_diag_netlog_flush();

	/* A second initialization would create competing VM/fence/ring ownership.
	 * Fail closed; reset recovery must operate on the retained device instead. */
	if (rae_adev)
		return -EBUSY;
	/* The curated real-driver closure currently targets Athena's Phoenix APU.
	 * Reject other ASICs before any MMIO so unsupported XCP/display/video paths
	 * cannot fall through generated off-path stubs. */
	if (vendor != 0x1002 || device != 0x15bf)
		return -ENODEV;

	pdev = kzalloc(sizeof(*pdev), GFP_KERNEL);
	if (!pdev)
		return -ENOMEM;
	/* A real pdev is never bus-less: amdgpu_acpi_vfct_bios matches the VFCT
	 * VBIOS image against pdev->bus->number + PCI_SLOT/PCI_FUNC(pdev->devfn).
	 * Leaving bus NULL was a NULL deref right after IP discovery (found
	 * off-target by tools/amdgpu_hostrun, 2026-07-08). */
	bus = kzalloc(sizeof(*bus), GFP_KERNEL);
	if (!bus) {
		kfree(pdev);
		return -ENOMEM;
	}
	bus->number    = pci_bus;
	pdev->bus      = bus;
	pdev->devfn    = pci_devfn;
	pdev->vendor   = vendor;
	pdev->device   = device;
	pdev->revision = revision;
	rae_set_bar(pdev, 0, bar0_phys, bar0_size);
	rae_set_bar(pdev, 2, bar2_phys, bar2_size);
	rae_set_bar(pdev, 5, bar5_phys, bar5_size);

	adev = kzalloc(sizeof(*adev), GFP_KERNEL);
	if (!adev) {
		kfree(pdev);
		return -ENOMEM;
	}
	adev->dev  = &pdev->dev;
	adev->pdev = pdev;
	{ extern void rae_dbg_ptrs(unsigned long,unsigned long,unsigned long); rae_dbg_ptrs(0xADE7, (unsigned long)adev, sizeof(*adev)); }

	pci_enable_device(pdev);

	/* run subsystem module_init() initcalls (drm_buddy's slab cache etc.)
	 * before amdgpu_device_init reaches drm_buddy_init during gmc sw_init. */
	rae_run_initcalls();

	/* initialise the drm_device fields the skipped drm_dev_init would set
	 * (vma_offset_manager + anon_inode) — amdgpu_ttm_init derefs them. */
	rae_amdgpu_setup_ddev(adev);
	r = rae_amdgpu_render_device_init(adev);
	if (r)
		return r;

	/* the real thing: *_ip_early_init -> device_init -> *_ip_init (GMC/PSP/SMU/
	 * IH/GFX/MES) — the complete init that establishes the broader power/clock/
	 * RLC/IH state the MES microengine needs to stay alive past 0x7654. */
	r = amdgpu_driver_load_kms(adev, flags);
	if (!r)
		rae_adev = adev;
	return r;
}

/* First real UAPI probe for the retained device. This deliberately calls the
 * upstream AMDGPU_INFO handler, including its copy_to_user path, rather than
 * synthesizing an answer in Rust. AMDGPU_INFO_ACCEL_WORKING does not require
 * per-file state, so it is safe before render-node client plumbing exists. */
int rae_amdgpu_info_accel_working(u32 *working)
{
	struct drm_amdgpu_info info = { 0 };

	if (!rae_adev)
		return -ENODEV;
	if (!working)
		return -EINVAL;

	*working = 0;
	info.return_pointer = (u64)(unsigned long)working;
	info.return_size = sizeof(*working);
	info.query = AMDGPU_INFO_ACCEL_WORKING;
	return amdgpu_info_ioctl(adev_to_drm(rae_adev), &info, NULL);
}
