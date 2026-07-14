/* SPDX-License-Identifier: MPL-2.0 */
/*
 * bringup_drm.c — minimal drm_device field setup for the headless bring-up
 * (MPL-2.0, original work).
 *
 * The direct daemon entry (rae_amdgpu_device_init -> amdgpu_driver_load_kms ->
 * amdgpu_device_init) constructs `struct amdgpu_device` with kzalloc and does
 * NOT run the drm_dev_alloc / drm_dev_init path that normally initialises the
 * embedded `struct drm_device` (adev->ddev). Two of those uninitialised fields
 * are dereferenced during amdgpu_ttm_init -> ttm_device_init:
 *
 *   - ddev->vma_offset_manager : TTM and GEM use the real drm_vma_manager to
 *     allocate unforgeable mmap offsets. A non-NULL initialized manager is
 *     required before either kernel BO creation or render-client mmap.
 *
 *   - ddev->anon_inode->i_mapping : passed as TTM's address_space. Nothing in the
 *     bring-up sets anon_inode; provide a real inode+address_space so the deref
 *     is deterministic rather than relying on a readable low page.
 *
 * This is bring-up infrastructure we own (not a patch to upstream amdgpu). It is
 * called from bringup_entry.c BEFORE amdgpu_driver_load_kms(). License boundary
 * (../README.md): API surface only.
 */
#include <linux/slab.h>
#include <linux/fs.h>
#include <drm/drm_device.h>
#include <drm/drm_vma_manager.h>
#include "amdgpu.h"

/*
 * Run the module_init() initcalls of the compiled drm-core subsystems. Linux
 * runs these at boot; the headless daemon has no module loader. module_init()
 * (see <linux/module.h>) emits a named extern wrapper rae_ic_<fn> around each
 * file-static initcall, which we invoke here — drm_buddy's creates the block
 * slab cache the VRAM allocator needs, drm_sched_fence's the scheduler-fence
 * slab the ring/MES submit path uses. Add a line here when a new module_init
 * subsystem enters the bring-up subset.
 */
extern int rae_ic_drm_buddy_module_init(void);
extern int rae_ic_drm_sched_fence_slab_init(void);

void rae_run_initcalls(void)
{
	rae_ic_drm_buddy_module_init();
	rae_ic_drm_sched_fence_slab_init();
}

void rae_amdgpu_setup_ddev(struct amdgpu_device *adev)
{
	struct drm_device *ddev = adev_to_drm(adev);

	/* Keep firmware BOs in contiguous VRAM. TTM allocates a large GTT BO as
	 * several compound-page chunks, and the current vmap seam correctly fails
	 * closed rather than pretending those discontiguous pages form one CPU
	 * mapping. Athena's VRAM is UMA carveout RAM; the host maps large CPU-write /
	 * GPU-read carveout ranges WB while retaining UC for small ring/fence
	 * readback. That makes the contiguous firmware path both mappable and fast. */
	adev->debug_use_vram_fw_buf = true;

	if (!ddev->vma_offset_manager) {
		struct drm_vma_offset_manager *mgr =
			kzalloc(sizeof(*mgr), GFP_KERNEL);
		if (mgr) {
			drm_vma_offset_manager_init(mgr,
						    DRM_FILE_PAGE_OFFSET_START,
						    DRM_FILE_PAGE_OFFSET_SIZE);
			ddev->vma_offset_manager = mgr;
		}
	}

	if (!ddev->anon_inode) {
		struct inode *ino = kzalloc(sizeof(*ino), GFP_KERNEL);
		struct address_space *mapping =
			kzalloc(sizeof(*mapping), GFP_KERNEL);
		if (ino && mapping) {
			ino->i_mapping = mapping;
			ddev->anon_inode = ino;
		} else {
			kfree(ino);
			kfree(mapping);
		}
	}
}

/*
 * RAS (ECC error reporting) per-IP sw_init stubs. amdgpu_gmc_ras_sw_init() chains
 * these; each per-block .c is out of the bring-up subset (RAS is not needed for
 * GFX/MES bring-up), so they resolve to inert weak stubs whose garbage return
 * value fails gmc_v11_0_sw_init. Upstream, these return 0 early when RAS is
 * unsupported for the block — which is exactly our state (no RAS registered). So
 * return 0 (block not set up) rather than leaking eax. amdgpu_hdp_ras_sw_init is
 * compiled real and already returns 0. Signatures match <amdgpu.h>.
 */
/* amdgpu_vce_required_gart_pages: the VCE .c is out of the bring-up subset (Phoenix
 * has VCN, not VCE). Upstream returns 0 for VCE2+/non-SI. The weak stub leaked
 * garbage into amdgpu_gtt_mgr_init's `start`, underflowing the GART drm_mm range so
 * every amdgpu_ttm_alloc_gart bind failed -ENOSPC (writeback/sdma_access BOs). */
u32 amdgpu_vce_required_gart_pages(struct amdgpu_device *adev) { (void)adev; return 0; }

int amdgpu_umc_ras_sw_init(struct amdgpu_device *adev)      { (void)adev; return 0; }
int amdgpu_mmhub_ras_sw_init(struct amdgpu_device *adev)    { (void)adev; return 0; }
int amdgpu_mca_mp0_ras_sw_init(struct amdgpu_device *adev)  { (void)adev; return 0; }
int amdgpu_mca_mp1_ras_sw_init(struct amdgpu_device *adev)  { (void)adev; return 0; }
int amdgpu_mca_mpio_ras_sw_init(struct amdgpu_device *adev) { (void)adev; return 0; }
int amdgpu_xgmi_ras_sw_init(struct amdgpu_device *adev)     { (void)adev; return 0; }
