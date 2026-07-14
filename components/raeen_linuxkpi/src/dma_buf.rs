//! `dma_buf` — cross-driver buffer sharing (PRIME import/export).
//!
//! A `dma_buf` is an exporter-owned buffer that other drivers attach to and map
//! for DMA. The importer side — which is what amdgpu uses to import another
//! device's buffer — is almost entirely DISPATCH: `dma_buf_*_attachment`,
//! `dma_buf_pin`, etc. just call through the exporter's `dma_buf_ops` vtable.
//! The shim implements that dispatch faithfully against BTF-verified layouts.
//!
//! **Layouts BTF-verified** (`pahole`, Linux 6.6): `dma_buf` (size 232;
//! attachments@16, ops@32, resv@120), `dma_buf_attachment` (size 72; dmabuf@0
//! dev@8 node@16 sgt@32 dir@40 importer_ops@48 importer_priv@56), `dma_buf_ops`
//! (size 104; attach@8 … map_dma_buf@40 unmap_dma_buf@48), `dma_buf_attach_ops`
//! (size 16; move_notify@8). Const-assert guards at the bottom.
//!
//! **Host-dependent edges (documented, not faked):** `dma_buf_get(fd)` and
//! `dma_buf_put` are file-descriptor / `struct file` based — cross-process
//! sharing needs the host to broker the fd (a future `SYS_LINUXKPI_DMABUF_GET`).
//! Until then `dma_buf_get` returns `ERR_PTR(-ENODEV)` and `dma_buf_put` is a
//! no-op (the exporter/host owns lifetime). The dispatch path — the part an
//! importer actually drives — is fully functional.

use crate::dma_fence::{list_add_tail, list_del_init, list_init, ListHead};
use crate::mm;
use crate::scatterlist::SgTable;
use core::ptr;

const ENODEV: isize = -19;

/// `struct dma_buf` — only the importer-relevant fields are named; the tail is
/// reserved to match the verified 232-byte size (we never allocate one — the
/// exporter does — so we only read `ops`/`attachments`).
#[repr(C)]
pub struct DmaBuf {
    pub size: usize,           // @0
    pub file: *mut u8,         // @8
    pub attachments: ListHead, // @16
    pub ops: *const DmaBufOps, // @32
    _reserved: [u8; 192],      // @40 .. 232 (vmapping_counter … cb_out)
}

/// `struct dma_buf_attachment`.
#[repr(C)]
pub struct DmaBufAttachment {
    pub dmabuf: *mut DmaBuf,                  // @0
    pub dev: *mut u8,                         // @8
    pub node: ListHead,                       // @16
    pub sgt: *mut SgTable,                    // @32
    pub dir: i32,                             // @40
    pub peer2peer: bool,                      // @44
    pub importer_ops: *const DmaBufAttachOps, // @48
    pub importer_priv: *mut u8,               // @56
    pub priv_: *mut u8,                       // @64
}

/// `struct dma_buf_ops` — the exporter vtable (importer-relevant slots named).
#[repr(C)]
pub struct DmaBufOps {
    pub cache_sgt_mapping: bool, // @0
    pub attach: Option<extern "C" fn(*mut DmaBuf, *mut DmaBufAttachment) -> i32>, // @8
    pub detach: Option<extern "C" fn(*mut DmaBuf, *mut DmaBufAttachment)>, // @16
    pub pin: Option<extern "C" fn(*mut DmaBufAttachment) -> i32>, // @24
    pub unpin: Option<extern "C" fn(*mut DmaBufAttachment)>, // @32
    pub map_dma_buf: Option<extern "C" fn(*mut DmaBufAttachment, i32) -> *mut SgTable>, // @40
    pub unmap_dma_buf: Option<extern "C" fn(*mut DmaBufAttachment, *mut SgTable, i32)>, // @48
    pub release: Option<extern "C" fn(*mut DmaBuf)>, // @56
    pub begin_cpu_access: Option<extern "C" fn(*mut DmaBuf, i32) -> i32>, // @64
    pub end_cpu_access: Option<extern "C" fn(*mut DmaBuf, i32) -> i32>, // @72
    pub mmap: Option<extern "C" fn(*mut DmaBuf, *mut u8) -> i32>, // @80
    pub vmap: Option<extern "C" fn(*mut DmaBuf, *mut u8) -> i32>, // @88
    pub vunmap: Option<extern "C" fn(*mut DmaBuf, *mut u8)>, // @96
}

/// `struct dma_buf_attach_ops` — importer callbacks.
#[repr(C)]
pub struct DmaBufAttachOps {
    pub allow_peer2peer: bool,                                     // @0
    pub move_notify: Option<extern "C" fn(*mut DmaBufAttachment)>, // @8
}

#[inline]
unsafe fn ops_of(dmabuf: *mut DmaBuf) -> *const DmaBufOps {
    if dmabuf.is_null() {
        ptr::null()
    } else {
        (*dmabuf).ops
    }
}

/// `dma_buf_dynamic_attach(dmabuf, dev, importer_ops, importer_priv)` — allocate
/// an attachment, link it, and run the exporter's `attach`. Returns the
/// attachment, or `ERR_PTR(-ENOMEM)` / the exporter's error.
#[no_mangle]
pub unsafe extern "C" fn dma_buf_dynamic_attach(
    dmabuf: *mut DmaBuf,
    dev: *mut u8,
    importer_ops: *const DmaBufAttachOps,
    importer_priv: *mut u8,
) -> *mut DmaBufAttachment {
    if dmabuf.is_null() {
        return (-22isize) as *mut DmaBufAttachment; // ERR_PTR(-EINVAL)
    }
    let attach = mm::kzalloc(core::mem::size_of::<DmaBufAttachment>(), 0) as *mut DmaBufAttachment;
    if attach.is_null() {
        return (-12isize) as *mut DmaBufAttachment; // ERR_PTR(-ENOMEM)
    }
    (*attach).dmabuf = dmabuf;
    (*attach).dev = dev;
    (*attach).importer_ops = importer_ops;
    (*attach).importer_priv = importer_priv;
    list_init(core::ptr::addr_of_mut!((*attach).node));

    // The exporter inits attachments in dma_buf_export; defensively init a
    // zeroed head so a freshly-zeroed mock/buffer is safe too.
    let head = core::ptr::addr_of_mut!((*dmabuf).attachments);
    if (*head).next.is_null() {
        list_init(head);
    }
    list_add_tail(core::ptr::addr_of_mut!((*attach).node), head);

    let ops = ops_of(dmabuf);
    if !ops.is_null() {
        if let Some(at) = (*ops).attach {
            let rc = at(dmabuf, attach);
            if rc != 0 {
                list_del_init(core::ptr::addr_of_mut!((*attach).node));
                mm::kfree(attach as *mut u8);
                return (rc as isize) as *mut DmaBufAttachment;
            }
        }
    }
    attach
}

/// `dma_buf_attach(dmabuf, dev)` — static attach (no importer ops).
#[no_mangle]
pub unsafe extern "C" fn dma_buf_attach(
    dmabuf: *mut DmaBuf,
    dev: *mut u8,
) -> *mut DmaBufAttachment {
    dma_buf_dynamic_attach(dmabuf, dev, ptr::null(), ptr::null_mut())
}

/// `dma_buf_detach(dmabuf, attach)` — run the exporter's `detach`, unlink, free.
#[no_mangle]
pub unsafe extern "C" fn dma_buf_detach(dmabuf: *mut DmaBuf, attach: *mut DmaBufAttachment) {
    if attach.is_null() {
        return;
    }
    let ops = ops_of(dmabuf);
    if !ops.is_null() {
        if let Some(de) = (*ops).detach {
            de(dmabuf, attach);
        }
    }
    list_del_init(core::ptr::addr_of_mut!((*attach).node));
    mm::kfree(attach as *mut u8);
}

/// `dma_buf_map_attachment(attach, dir)` — dispatch to the exporter's
/// `map_dma_buf`; caches the result in `attach->sgt`. Returns the sg_table or
/// `ERR_PTR(-EINVAL)` if the exporter provides none.
#[no_mangle]
pub unsafe extern "C" fn dma_buf_map_attachment(
    attach: *mut DmaBufAttachment,
    dir: i32,
) -> *mut SgTable {
    if attach.is_null() {
        return (-22isize) as *mut SgTable;
    }
    let ops = ops_of((*attach).dmabuf);
    if ops.is_null() {
        return (-22isize) as *mut SgTable;
    }
    match (*ops).map_dma_buf {
        Some(map) => {
            let sgt = map(attach, dir);
            (*attach).sgt = sgt;
            (*attach).dir = dir;
            sgt
        }
        None => (-22isize) as *mut SgTable,
    }
}

/// `dma_buf_map_attachment_unlocked` — same in the cooperative daemon.
#[no_mangle]
pub unsafe extern "C" fn dma_buf_map_attachment_unlocked(
    attach: *mut DmaBufAttachment,
    dir: i32,
) -> *mut SgTable {
    dma_buf_map_attachment(attach, dir)
}

/// `dma_buf_unmap_attachment(attach, sgt, dir)`.
#[no_mangle]
pub unsafe extern "C" fn dma_buf_unmap_attachment(
    attach: *mut DmaBufAttachment,
    sgt: *mut SgTable,
    dir: i32,
) {
    if attach.is_null() {
        return;
    }
    let ops = ops_of((*attach).dmabuf);
    if !ops.is_null() {
        if let Some(unmap) = (*ops).unmap_dma_buf {
            unmap(attach, sgt, dir);
        }
    }
    (*attach).sgt = ptr::null_mut();
}
#[no_mangle]
pub unsafe extern "C" fn dma_buf_unmap_attachment_unlocked(
    attach: *mut DmaBufAttachment,
    sgt: *mut SgTable,
    dir: i32,
) {
    dma_buf_unmap_attachment(attach, sgt, dir);
}

/// `dma_buf_pin(attach)` — dispatch to the exporter (no-op success if absent).
#[no_mangle]
pub unsafe extern "C" fn dma_buf_pin(attach: *mut DmaBufAttachment) -> i32 {
    if attach.is_null() {
        return -22;
    }
    let ops = ops_of((*attach).dmabuf);
    if !ops.is_null() {
        if let Some(pin) = (*ops).pin {
            return pin(attach);
        }
    }
    0
}

/// `dma_buf_unpin(attach)`.
#[no_mangle]
pub unsafe extern "C" fn dma_buf_unpin(attach: *mut DmaBufAttachment) {
    if attach.is_null() {
        return;
    }
    let ops = ops_of((*attach).dmabuf);
    if !ops.is_null() {
        if let Some(unpin) = (*ops).unpin {
            unpin(attach);
        }
    }
}

/// `dma_buf_move_notify(dmabuf)` — tell every importer with a `move_notify` that
/// the backing storage moved (so it re-maps).
#[no_mangle]
pub unsafe extern "C" fn dma_buf_move_notify(dmabuf: *mut DmaBuf) {
    if dmabuf.is_null() {
        return;
    }
    let head = core::ptr::addr_of_mut!((*dmabuf).attachments);
    if (*head).next.is_null() {
        return;
    }
    let mut cur = (*head).next;
    while cur != head && !cur.is_null() {
        let next = (*cur).next;
        let attach =
            (cur as usize - core::mem::offset_of!(DmaBufAttachment, node)) as *mut DmaBufAttachment;
        let iops = (*attach).importer_ops;
        if !iops.is_null() {
            if let Some(mv) = (*iops).move_notify {
                mv(attach);
            }
        }
        cur = next;
    }
}

/// `dma_buf_get(fd)` — resolve a dma_buf fd. Cross-process fd brokering is not
/// wired yet (see module note), so this returns `ERR_PTR(-ENODEV)`.
#[no_mangle]
pub extern "C" fn dma_buf_get(_fd: i32) -> *mut DmaBuf {
    ENODEV as *mut DmaBuf
}

/// `dma_buf_put(dmabuf)` — file-refcount based; lifetime is owned by the
/// exporter/host in the daemon model, so this is a no-op.
#[no_mangle]
pub extern "C" fn dma_buf_put(_dmabuf: *mut DmaBuf) {}

// ── compile-time layout guard (BTF: Linux 6.6 x86_64) ────────────────────────
const _: () = assert!(core::mem::size_of::<DmaBuf>() == 232);
const _: () = assert!(core::mem::offset_of!(DmaBuf, attachments) == 16);
const _: () = assert!(core::mem::offset_of!(DmaBuf, ops) == 32);
const _: () = assert!(core::mem::size_of::<DmaBufAttachment>() == 72);
const _: () = assert!(core::mem::offset_of!(DmaBufAttachment, node) == 16);
const _: () = assert!(core::mem::offset_of!(DmaBufAttachment, sgt) == 32);
const _: () = assert!(core::mem::offset_of!(DmaBufAttachment, importer_ops) == 48);
const _: () = assert!(core::mem::size_of::<DmaBufOps>() == 104);
const _: () = assert!(core::mem::offset_of!(DmaBufOps, map_dma_buf) == 40);
const _: () = assert!(core::mem::offset_of!(DmaBufOps, unmap_dma_buf) == 48);
const _: () = assert!(core::mem::size_of::<DmaBufAttachOps>() == 16);
const _: () = assert!(core::mem::offset_of!(DmaBufAttachOps, move_notify) == 8);
