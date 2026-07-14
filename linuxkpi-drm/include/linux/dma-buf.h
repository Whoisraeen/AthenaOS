/* SPDX-License-Identifier: MPL-2.0 */
/*
 * <linux/dma-buf.h> shim (MPL-2.0, original work).
 *
 * The cross-device buffer-sharing object (PRIME / GPU buffer import-export).
 * amdgpu exports its BOs as dma-bufs and imports peers' (XGMI, multi-GPU, and
 * the compositor share path). Without this shim `<linux/dma-buf.h>` LEAKS to the
 * host's UAPI header, which has no kernel `struct dma_buf` — hence the type must
 * be defined here for layout. The export/attach ops are backed by ath_linuxkpi
 * at M4. License boundary (../../README.md): API surface.
 */
#ifndef _LINUXKPI_LINUX_DMA_BUF_H
#define _LINUXKPI_LINUX_DMA_BUF_H

#include <linux/types.h>
#include <linux/dma-fence.h>
#include <linux/dma-resv.h>
#include <linux/scatterlist.h>
#include <linux/fs.h>
#include <linux/list.h>
#include <linux/iosys-map.h>
#include <linux/dma-direction.h>   /* enum dma_data_direction (used by value below) */
#include <linux/dma-mapping.h>     /* DMA_ATTR_* (dma_buf importers pass them) */

struct device;
struct dma_buf;
struct dma_buf_attachment;

struct dma_buf_ops {
	bool cache_sgt_mapping;
	int  (*attach)(struct dma_buf *, struct dma_buf_attachment *);
	void (*detach)(struct dma_buf *, struct dma_buf_attachment *);
	int  (*pin)(struct dma_buf_attachment *);
	void (*unpin)(struct dma_buf_attachment *);
	struct sg_table *(*map_dma_buf)(struct dma_buf_attachment *, enum dma_data_direction);
	void (*unmap_dma_buf)(struct dma_buf_attachment *, struct sg_table *, enum dma_data_direction);
	void (*release)(struct dma_buf *);
	int  (*begin_cpu_access)(struct dma_buf *, enum dma_data_direction);
	int  (*end_cpu_access)(struct dma_buf *, enum dma_data_direction);
	int  (*mmap)(struct dma_buf *, struct vm_area_struct *);
	int  (*vmap)(struct dma_buf *, struct iosys_map *);
	void (*vunmap)(struct dma_buf *, struct iosys_map *);
};

struct dma_buf {
	size_t                    size;
	struct file              *file;
	struct list_head          attachments;
	const struct dma_buf_ops *ops;
	void                     *vmap_ptr;
	const char               *exp_name;
	struct module            *owner;
	struct list_head          list_node;
	void                     *priv;
	struct dma_resv          *resv;
};

struct dma_buf_attach_ops {
	bool allow_peer2peer;
	void (*move_notify)(struct dma_buf_attachment *attach);
};

struct dma_buf_attachment {
	struct dma_buf               *dmabuf;
	struct device                *dev;
	struct list_head              node;
	struct sg_table              *sgt;
	enum dma_data_direction       dir;
	bool                          peer2peer;
	const struct dma_buf_attach_ops *importer_ops;
	void                         *importer_priv;
	void                         *priv;
};

struct dma_buf_export_info {
	const char               *exp_name;
	struct module            *owner;
	const struct dma_buf_ops *ops;
	size_t                    size;
	int                       flags;
	struct dma_resv          *resv;
	void                     *priv;
};

#define DEFINE_DMA_BUF_EXPORT_INFO(name) \
	struct dma_buf_export_info name = { .exp_name = __FILE__, .owner = (void *)0 }

/* no dynamic (move-notify) attachment during bring-up; importers see a pinned buf. */
static inline bool dma_buf_is_dynamic(struct dma_buf *dmabuf) { (void)dmabuf; return false; }

/* export / lifetime — backed by ath_linuxkpi (M4) */
void get_dma_buf(struct dma_buf *dmabuf);   /* take a reference (file refcount) */
struct dma_buf *dma_buf_export(const struct dma_buf_export_info *exp_info);
int  dma_buf_fd(struct dma_buf *dmabuf, int flags);
struct dma_buf *dma_buf_get(int fd);
void dma_buf_put(struct dma_buf *dmabuf);

/* attach / map — backed by ath_linuxkpi (M4) */
struct dma_buf_attachment *dma_buf_attach(struct dma_buf *dmabuf, struct device *dev);
struct dma_buf_attachment *dma_buf_dynamic_attach(struct dma_buf *dmabuf, struct device *dev,
						  const struct dma_buf_attach_ops *importer_ops,
						  void *importer_priv);
void dma_buf_detach(struct dma_buf *dmabuf, struct dma_buf_attachment *attach);
struct sg_table *dma_buf_map_attachment(struct dma_buf_attachment *attach, enum dma_data_direction dir);
struct sg_table *dma_buf_map_attachment_unlocked(struct dma_buf_attachment *attach,
						 enum dma_data_direction dir);
void dma_buf_unmap_attachment(struct dma_buf_attachment *attach, struct sg_table *sg_table,
			      enum dma_data_direction dir);
void dma_buf_unmap_attachment_unlocked(struct dma_buf_attachment *attach,
				       struct sg_table *sg_table,
				       enum dma_data_direction dir);
int  dma_buf_pin(struct dma_buf_attachment *attach);
void dma_buf_unpin(struct dma_buf_attachment *attach);
int  dma_buf_vmap(struct dma_buf *dmabuf, struct iosys_map *map);
void dma_buf_vunmap(struct dma_buf *dmabuf, struct iosys_map *map);

#endif /* _LINUXKPI_LINUX_DMA_BUF_H */
