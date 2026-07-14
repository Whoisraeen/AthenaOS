// SPDX-License-Identifier: MPL-2.0
/*
 * Retained-device DRM render client seam for amdgpud.
 *
 * The upstream AMDGPU ioctl handlers, GEM/TTM, VM, scheduler, syncobj and DRM
 * file core are compiled unchanged.  This file supplies the small piece that
 * Linux's PCI/DRM registration path normally constructs around them: one
 * render-only drm_driver, one stable render minor, and explicit client
 * open/ioctl/close entry points for the AthenaOS capability broker.
 */
#include <linux/err.h>
#include <linux/fs.h>
#include <linux/slab.h>

#include <drm/drm_device.h>
#include <drm/drm_drv.h>
#include <drm/drm_file.h>
#include <drm/drm_gem.h>
#include <drm/drm_ioctl.h>
#include <drm/drm_vma_manager.h>
#include <drm/ttm/ttm_tt.h>

#include "amdgpu.h"
#include "amdgpu_dma_buf.h"
#include "amdgpu_userq.h"

struct drm_file *drm_file_alloc(struct drm_minor *minor);

struct rae_render_client {
	struct file filp;
	struct drm_file *file;
	/* One GEM reference per successful broker mmap. Linux VMAs retain the GEM
	 * object until vm_close; the broker's first vertical slice releases these
	 * at render-file close (bounded, never a dangling physical mapping). */
	struct drm_gem_object *mapped[256];
	u32 mapped_count;
};

/* USERQ is not part of the observed RADV first-frame path.  Make the open path
 * take upstream's documented legacy-submit fallback instead of letting an
 * untyped weak stub claim that USERQ initialized successfully. */
int amdgpu_userq_mgr_init(struct amdgpu_userq_mgr *mgr,
			  struct drm_file *file, struct amdgpu_device *adev)
{
	(void)mgr;
	(void)file;
	(void)adev;
	return -EOPNOTSUPP;
}

/* Debugfs is not exposed by the sandboxed daemon.  This is diagnostic-only and
 * has no state that the VM/submit path consumes. */
void amdgpu_debugfs_vm_init(struct drm_file *file)
{
	(void)file;
}

static const struct drm_ioctl_desc rae_amdgpu_render_ioctls[] = {
	DRM_IOCTL_DEF_DRV(AMDGPU_GEM_CREATE, amdgpu_gem_create_ioctl,
			  DRM_AUTH | DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(AMDGPU_CTX, amdgpu_ctx_ioctl,
			  DRM_AUTH | DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(AMDGPU_VM, amdgpu_vm_ioctl,
			  DRM_AUTH | DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(AMDGPU_BO_LIST, amdgpu_bo_list_ioctl,
			  DRM_AUTH | DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(AMDGPU_FENCE_TO_HANDLE, amdgpu_cs_fence_to_handle_ioctl,
			  DRM_AUTH | DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(AMDGPU_GEM_MMAP, amdgpu_gem_mmap_ioctl,
			  DRM_AUTH | DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(AMDGPU_GEM_WAIT_IDLE, amdgpu_gem_wait_idle_ioctl,
			  DRM_AUTH | DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(AMDGPU_CS, amdgpu_cs_ioctl,
			  DRM_AUTH | DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(AMDGPU_INFO, amdgpu_info_ioctl,
			  DRM_AUTH | DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(AMDGPU_WAIT_CS, amdgpu_cs_wait_ioctl,
			  DRM_AUTH | DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(AMDGPU_WAIT_FENCES, amdgpu_cs_wait_fences_ioctl,
			  DRM_AUTH | DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(AMDGPU_GEM_METADATA, amdgpu_gem_metadata_ioctl,
			  DRM_AUTH | DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(AMDGPU_GEM_VA, amdgpu_gem_va_ioctl,
			  DRM_AUTH | DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(AMDGPU_GEM_OP, amdgpu_gem_op_ioctl,
			  DRM_AUTH | DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(AMDGPU_GEM_USERPTR, amdgpu_gem_userptr_ioctl,
			  DRM_AUTH | DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(AMDGPU_GEM_LIST_HANDLES, amdgpu_gem_list_handles_ioctl,
			  DRM_AUTH | DRM_RENDER_ALLOW),
};

static const struct file_operations rae_render_fops = {
	.fop_flags = FOP_UNSIGNED_OFFSET,
};

static const struct drm_driver rae_amdgpu_render_driver = {
	.driver_features = DRIVER_GEM | DRIVER_RENDER | DRIVER_SYNCOBJ |
			   DRIVER_SYNCOBJ_TIMELINE,
	.open = amdgpu_driver_open_kms,
	.postclose = amdgpu_driver_postclose_kms,
	.ioctls = rae_amdgpu_render_ioctls,
	.num_ioctls = ARRAY_SIZE(rae_amdgpu_render_ioctls),
	.fops = &rae_render_fops,
	.gem_prime_import = amdgpu_gem_prime_import,
	.name = "amdgpu",
	.desc = "AMD GPU",
	.major = 3,
	.minor = 64,
	.patchlevel = 0,
};

static struct device rae_render_kdev;
static struct drm_minor rae_render_minor;
static bool rae_render_ready;

int rae_amdgpu_render_device_init(struct amdgpu_device *adev)
{
	struct drm_device *ddev;
	int r;

	if (!adev)
		return -EINVAL;
	if (rae_render_ready)
		return -EBUSY;

	ddev = adev_to_drm(adev);
	ddev->dev = adev->dev;
	ddev->driver = &rae_amdgpu_render_driver;
	ddev->driver_features = ~0U;

	INIT_LIST_HEAD(&ddev->managed.resources);
	spin_lock_init(&ddev->managed.lock);
	INIT_LIST_HEAD(&ddev->filelist);
	INIT_LIST_HEAD(&ddev->filelist_internal);
	INIT_LIST_HEAD(&ddev->clientlist);
	INIT_LIST_HEAD(&ddev->client_sysrq_list);
	INIT_LIST_HEAD(&ddev->vblank_event_list);
	spin_lock_init(&ddev->event_lock);
	mutex_init(&ddev->gem_lru_mutex);
	mutex_init(&ddev->filelist_mutex);
	mutex_init(&ddev->clientlist_mutex);
	mutex_init(&ddev->master_mutex);

	if (!ddev->anon_inode || !ddev->vma_offset_manager)
		return -ENOMEM;

	rae_render_kdev.devt = (226U << 20) | 128U;
	rae_render_minor.index = 128;
	rae_render_minor.type = DRM_MINOR_RENDER;
	rae_render_minor.kdev = &rae_render_kdev;
	rae_render_minor.dev = ddev;
	ddev->render = &rae_render_minor;

	r = drm_gem_init(ddev);
	if (r)
		return r;

	rae_render_ready = true;
	return 0;
}

void *rae_amdgpu_render_open(void)
{
	struct rae_render_client *client;
	struct drm_file *file;

	if (!rae_render_ready)
		return ERR_PTR(-ENODEV);

	client = kzalloc(sizeof(*client), GFP_KERNEL);
	if (!client)
		return ERR_PTR(-ENOMEM);

	file = drm_file_alloc(&rae_render_minor);
	if (IS_ERR(file)) {
		kfree(client);
		return file;
	}

	client->filp.f_op = &rae_render_fops;
	client->filp.f_mapping = rae_render_minor.dev->anon_inode->i_mapping;
	client->filp.private_data = file;
	client->file = file;
	file->filp = &client->filp;
	return client;
}

int rae_amdgpu_render_ioctl(void *opaque, unsigned int cmd, void *arg)
{
	struct rae_render_client *client = opaque;

	if (!client || IS_ERR(client) || !client->file)
		return -EBADF;
	return (int)drm_ioctl(&client->filp, cmd, (unsigned long)arg);
}

void rae_amdgpu_render_close(void *opaque)
{
	struct rae_render_client *client = opaque;
	struct amdgpu_fpriv *fpriv;
	u32 i;

	if (!client || IS_ERR(client))
		return;
	fpriv = client->file ? client->file->driver_priv : NULL;
	if (fpriv) {
		fpriv->evf_mgr.fd_closing = true;
		amdgpu_eviction_fence_destroy(&fpriv->evf_mgr);
	}
	for (i = 0; i < client->mapped_count; i++)
		drm_gem_object_put(client->mapped[i]);
	drm_file_free(client->file);
	kfree(client);
}

/* Resolve a GEM fake mmap offset through the exact upstream per-device VMA
 * manager and export the physical pages backing a GTT BO. The caller still has
 * to validate these pages against amdgpud's host-owned DMA regions before
 * mapping them into a client. VRAM/fixed-memory BOs deliberately fail closed;
 * the minimum Mesa upload/command path allocates CPU-visible GTT BOs. */
int rae_amdgpu_render_mmap_pages(void *opaque, u64 offset, u64 length,
				 u64 *pages_out, u32 pages_cap)
{
	struct rae_render_client *client = opaque;
	struct drm_vma_offset_manager *mgr;
	struct drm_vma_offset_node *node;
	struct drm_gem_object *obj = NULL;
	struct amdgpu_bo *bo;
	unsigned long start, pages;
	u32 i;
	int r = 0;

	if (!client || IS_ERR(client) || !client->file || !length ||
	    (offset & (PAGE_SIZE - 1)) || (length & (PAGE_SIZE - 1)))
		return -EINVAL;
	pages = length >> PAGE_SHIFT;
	if (pages > pages_cap || !pages_out)
		return -E2BIG;
	start = offset >> PAGE_SHIFT;
	mgr = client->file->minor->dev->vma_offset_manager;

	drm_vma_offset_lock_lookup(mgr);
	node = drm_vma_offset_exact_lookup_locked(mgr, start, pages);
	if (node) {
		obj = container_of(node, struct drm_gem_object, vma_node);
		if (!kref_get_unless_zero(&obj->refcount))
			obj = NULL;
	}
	drm_vma_offset_unlock_lookup(mgr);
	if (!obj)
		return -EINVAL;
	if (!drm_vma_node_is_allowed(node, client->file)) {
		r = -EACCES;
		goto out_put;
	}

	bo = gem_to_amdgpu_bo(obj);
	if (!bo->tbo.ttm || bo->tbo.ttm->num_pages < pages) {
		r = -ENXIO; /* fixed/VRAM BO or not populated: never fake a mapping */
		goto out_put;
	}
	for (i = 0; i < pages; i++) {
		struct page *page = bo->tbo.ttm->pages[i];
		if (!page) {
			r = -EAGAIN;
			goto out_put;
		}
		pages_out[i] = page_to_phys(page);
		if (!pages_out[i] || (pages_out[i] & (PAGE_SIZE - 1))) {
			r = -EIO;
			goto out_put;
		}
	}
	if (client->mapped_count >= ARRAY_SIZE(client->mapped)) {
		r = -ENOSPC;
		goto out_put;
	}
	client->mapped[client->mapped_count++] = obj;
	return (int)pages; /* the client mapping now owns this GEM reference */

out_put:
	drm_gem_object_put(obj);
	return r;
}
