//! Kernel-owned `/dev/dri/renderD128` broker.
//!
//! The render client never enters the privileged `amdgpud` address space and
//! the daemon never dereferences a client pointer. Arguments cross this seam as
//! bounded copies; commands with nested pointers require an explicit marshaller
//! and otherwise fail closed.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::Arc;
use alloc::vec::Vec;
use ath_abi::drm_service as wire;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

const ENODEV: i64 = -19;
const EFAULT: i64 = -14;
const EINVAL: i64 = -22;
const ENOTTY: i64 = -25;
const E2BIG: i64 = -7;

const DRM_IOCTL_AMDGPU_INFO: u32 = 0x4020_6445;
const DRM_IOCTL_VERSION: u32 = 0xc040_6400;

#[derive(Clone)]
struct Request {
    header: wire::RequestHeader,
    payload: Vec<u8>,
}

struct Pending {
    request_id: u64,
    user_arg: u64,
    arg_len: usize,
    copy_flat_out: bool,
    aux: Vec<AuxCopy>,
}

struct AuxCopy {
    pointer_field: usize,
    user_ptr: u64,
    payload_offset: usize,
    len: usize,
    copy_out: bool,
}

struct Response {
    status: i32,
    payload: Vec<u8>,
}

#[derive(Default)]
struct Broker {
    service_task: Option<u64>,
    service_device: Option<u64>,
    queue: VecDeque<Request>,
    waiting: BTreeMap<u64, Pending>,
    mmap_waiting: BTreeMap<u64, u64>,
    responses: BTreeMap<u64, Response>,
}

static BROKER: Mutex<Broker> = Mutex::new(Broker {
    service_task: None,
    service_device: None,
    queue: VecDeque::new(),
    waiting: BTreeMap::new(),
    mmap_waiting: BTreeMap::new(),
    responses: BTreeMap::new(),
});
static NEXT_CLIENT: AtomicU64 = AtomicU64::new(1);
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);
static DEFERRED_WAKES: Mutex<Vec<u64>> = Mutex::new(Vec::new());

pub struct RenderInode {
    client_id: u64,
}

impl RenderInode {
    pub fn open() -> Option<Arc<dyn crate::vfs::Inode>> {
        if !is_available() {
            return None;
        }
        let client_id = NEXT_CLIENT.fetch_add(1, Ordering::Relaxed);
        enqueue_control(wire::OP_OPEN, client_id);
        Some(Arc::new(Self { client_id }))
    }
}

impl Drop for RenderInode {
    fn drop(&mut self) {
        enqueue_control(wire::OP_CLOSE, self.client_id);
    }
}

impl crate::vfs::Inode for RenderInode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> usize {
        0
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> usize {
        0
    }
    fn size(&self) -> usize {
        0
    }
    fn render_client_id(&self) -> Option<u64> {
        Some(self.client_id)
    }
}

pub fn is_available() -> bool {
    BROKER.lock().service_task.is_some()
}

/// Task-exit cleanup. Called under the scheduler lock, so waking is deferred
/// until the next syscall entry; table cleanup itself never re-enters the
/// scheduler. A crashed amdgpud revokes the node and completes every in-flight
/// operation with ENODEV, allowing the supervisor to register a replacement.
pub fn cleanup_task(task_id: u64) {
    let mut broker = BROKER.lock();
    if broker.service_task == Some(task_id) {
        broker.service_task = None;
        broker.service_device = None;
        broker.queue.clear();
        let ioctl_waiters: Vec<(u64, u64)> = broker
            .waiting
            .iter()
            .map(|(task, pending)| (*task, pending.request_id))
            .collect();
        let mmap_waiters: Vec<(u64, u64)> = broker
            .mmap_waiting
            .iter()
            .map(|(task, request)| (*task, *request))
            .collect();
        for (task, request_id) in ioctl_waiters.into_iter().chain(mmap_waiters) {
            broker.responses.insert(
                request_id,
                Response {
                    status: ENODEV as i32,
                    payload: Vec::new(),
                },
            );
            DEFERRED_WAKES.lock().push(task);
        }
    } else {
        if let Some(pending) = broker.waiting.remove(&task_id) {
            broker.responses.remove(&pending.request_id);
        }
        if let Some(request_id) = broker.mmap_waiting.remove(&task_id) {
            broker.responses.remove(&request_id);
        }
    }
}

pub fn drain_deferred_wakes() {
    let wakes = core::mem::take(&mut *DEFERRED_WAKES.lock());
    for task in wakes {
        crate::scheduler::wake_thread(crate::task::TaskId::from_raw(task));
    }
}

fn enqueue_control(op: u32, client_id: u64) {
    let request_id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
    let mut broker = BROKER.lock();
    if broker.service_task.is_none() {
        return;
    }
    broker.queue.push_back(Request {
        header: wire::RequestHeader {
            version: wire::VERSION,
            op,
            request_id,
            client_id,
            ..wire::RequestHeader::default()
        },
        payload: Vec::new(),
    });
}

/// Register the retained upstream amdgpu object graph as the render service.
/// The LinuxKPI host proves that `device_handle` is an AMD display device owned
/// by the calling task; a second live service is rejected.
pub fn sys_register(device_handle: u64) -> u64 {
    let Some(task) = crate::scheduler::current_task_id() else {
        return wire::ERR_DENIED;
    };
    if !crate::linuxkpi_host::caller_owns_amd_gpu(device_handle) {
        return wire::ERR_DENIED;
    }
    let mut broker = BROKER.lock();
    match broker.service_task {
        Some(owner) if owner != task.raw() => wire::ERR_BUSY,
        _ => {
            broker.service_task = Some(task.raw());
            broker.service_device = Some(device_handle);
            crate::serial_println!(
                "[drm] /dev/dri/renderD128 online: amdgpud task={} lkpi_device={}",
                task.raw(),
                device_handle
            );
            0
        }
    }
}

/// Non-blocking daemon fetch. amdgpud polls this from its resident service loop.
pub fn sys_fetch(header_ptr: u64, payload_ptr: u64, payload_cap: u64) -> u64 {
    let Some(caller) = crate::scheduler::current_task_id().map(|id| id.raw()) else {
        return wire::ERR_DENIED;
    };
    let request = {
        let mut broker = BROKER.lock();
        if broker.service_task != Some(caller) {
            return wire::ERR_DENIED;
        }
        broker.queue.pop_front()
    };
    let Some(request) = request else { return 0 };
    if request.payload.len() > payload_cap as usize {
        BROKER.lock().queue.push_front(request);
        return wire::ERR_INVALID;
    }
    let header_bytes = unsafe {
        core::slice::from_raw_parts(
            (&request.header as *const wire::RequestHeader).cast::<u8>(),
            core::mem::size_of::<wire::RequestHeader>(),
        )
    };
    if crate::uaccess::copy_to_user(header_ptr, header_bytes).is_err()
        || crate::uaccess::copy_to_user(payload_ptr, &request.payload).is_err()
    {
        BROKER.lock().queue.push_front(request);
        return wire::ERR_FAULT;
    }
    // +1 reserves zero for "queue empty" even for OPEN/CLOSE requests, whose
    // payload is legitimately empty.
    request.payload.len() as u64 + 1
}

pub fn sys_complete(request_id: u64, status_bits: u64, payload_ptr: u64, payload_len: u64) -> u64 {
    let Some(caller) = crate::scheduler::current_task_id().map(|id| id.raw()) else {
        return wire::ERR_DENIED;
    };
    if payload_len as usize > wire::MAX_PAYLOAD {
        return wire::ERR_INVALID;
    }
    let payload = match crate::uaccess::copy_from_user(payload_ptr, payload_len as usize) {
        Ok(bytes) => bytes,
        Err(_) => return wire::ERR_FAULT,
    };
    let waiter = {
        let mut broker = BROKER.lock();
        if broker.service_task != Some(caller) {
            return wire::ERR_DENIED;
        }
        let waiter = broker
            .waiting
            .iter()
            .find_map(|(task, pending)| (pending.request_id == request_id).then_some(*task))
            .or_else(|| {
                broker
                    .mmap_waiting
                    .iter()
                    .find_map(|(task, pending)| (*pending == request_id).then_some(*task))
            });
        let Some(waiter) = waiter else {
            return wire::ERR_INVALID;
        };
        broker.responses.insert(
            request_id,
            Response {
                status: status_bits as u32 as i32,
                payload,
            },
        );
        waiter
    };
    crate::scheduler::wake_thread(crate::task::TaskId::from_raw(waiter));
    0
}

pub enum IoctlAction {
    Complete(i64),
    BlockNew {
        request_id: u64,
        request: PreparedIoctl,
    },
    BlockExisting {
        request_id: u64,
    },
}

pub struct PreparedIoctl {
    task_id: u64,
    pending: Pending,
    request: Request,
}

pub enum MmapAction {
    Complete(Result<Vec<u64>, i64>),
    BlockNew {
        request_id: u64,
        request: PreparedMmap,
    },
    BlockExisting {
        request_id: u64,
    },
}

pub struct PreparedMmap {
    task_id: u64,
    request_id: u64,
    request: Request,
}

pub fn prepare_mmap(task_id: u64, client_id: u64, offset: u64, length: u64) -> MmapAction {
    if length == 0 || offset & 4095 != 0 || length & 4095 != 0 {
        return MmapAction::Complete(Err(EINVAL));
    }
    {
        let mut broker = BROKER.lock();
        let (Some(service_task), Some(service_device)) =
            (broker.service_task, broker.service_device)
        else {
            return MmapAction::Complete(Err(ENODEV));
        };
        if let Some(request_id) = broker.mmap_waiting.get(&task_id).copied() {
            let Some(response) = broker.responses.remove(&request_id) else {
                return MmapAction::BlockExisting { request_id };
            };
            broker.mmap_waiting.remove(&task_id);
            drop(broker);
            if response.status != 0 {
                return MmapAction::Complete(Err(response.status as i64));
            }
            let count = (length >> 12) as usize;
            if response.payload.len() != count.saturating_mul(8) {
                return MmapAction::Complete(Err(EINVAL));
            }
            let mut pages = Vec::with_capacity(count);
            for chunk in response.payload.chunks_exact(8) {
                let phys = u64::from_le_bytes(chunk.try_into().unwrap());
                if phys & 4095 != 0
                    || !crate::linuxkpi_host::owned_dma_range(
                        service_device,
                        service_task,
                        phys,
                        4096,
                    )
                {
                    return MmapAction::Complete(Err(EFAULT));
                }
                pages.push(phys);
            }
            return MmapAction::Complete(Ok(pages));
        }
    }

    let request_id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
    let mut payload = Vec::with_capacity(16);
    payload.extend_from_slice(&offset.to_le_bytes());
    payload.extend_from_slice(&length.to_le_bytes());
    MmapAction::BlockNew {
        request_id,
        request: PreparedMmap {
            task_id,
            request_id,
            request: Request {
                header: wire::RequestHeader {
                    version: wire::VERSION,
                    op: wire::OP_MMAP,
                    request_id,
                    client_id,
                    payload_len: 16,
                    ..wire::RequestHeader::default()
                },
                payload,
            },
        },
    }
}

pub fn enqueue_mmap(prepared: PreparedMmap) {
    let mut broker = BROKER.lock();
    if broker.service_task.is_none() {
        return;
    }
    broker
        .mmap_waiting
        .insert(prepared.task_id, prepared.request_id);
    broker.queue.push_back(prepared.request);
}

/// Poll a prior request or prepare a new bounded ioctl transaction.
pub fn prepare_ioctl(task_id: u64, client_id: u64, cmd: u32, user_arg: u64) -> IoctlAction {
    {
        let mut broker = BROKER.lock();
        if broker.service_task.is_none() {
            return IoctlAction::Complete(ENODEV);
        }
        if let Some(pending) = broker.waiting.get(&task_id) {
            let request_id = pending.request_id;
            if let Some(response) = broker.responses.remove(&request_id) {
                let pending = broker.waiting.remove(&task_id).unwrap();
                drop(broker);
                return IoctlAction::Complete(finish_response(pending, response));
            }
            return IoctlAction::BlockExisting { request_id };
        }
    }

    let resolved = match ath_render_broker::dispatch(cmd) {
        Ok(spec) => spec,
        Err(error) => return IoctlAction::Complete(error.errno() as i64),
    };

    // These commands contain client pointers beyond the flat ioctl record.
    // Each needs its dedicated marshaller; never pass those pointers through as
    // if they were daemon-local. INFO and VERSION are handled explicitly below.
    if matches!(resolved.name, "AMDGPU_GEM_USERPTR" | "AMDGPU_WAIT_FENCES") {
        return IoctlAction::Complete(ENOTTY);
    }
    let arg_len = resolved.size as usize;
    if arg_len > wire::MAX_PAYLOAD {
        return IoctlAction::Complete(E2BIG);
    }
    let mut payload = match crate::uaccess::copy_from_user(user_arg, arg_len) {
        Ok(bytes) => bytes,
        Err(_) => return IoctlAction::Complete(EFAULT),
    };
    let mut flags = 0u32;
    let mut aux = Vec::new();
    if cmd == DRM_IOCTL_AMDGPU_INFO {
        if arg_len != 32 {
            return IoctlAction::Complete(EINVAL);
        }
        let user_ptr = u64::from_le_bytes(payload[0..8].try_into().unwrap());
        let len = u32::from_le_bytes(payload[8..12].try_into().unwrap()) as usize;
        if append_aux(&mut payload, &mut aux, 0, user_ptr, len).is_err() {
            return IoctlAction::Complete(E2BIG);
        }
        flags |= wire::FLAG_INFO_AUX;
    } else if cmd == DRM_IOCTL_VERSION {
        if arg_len != 64 {
            return IoctlAction::Complete(EINVAL);
        }
        for (len_off, ptr_off) in [(16usize, 24usize), (32, 40), (48, 56)] {
            let len =
                u64::from_le_bytes(payload[len_off..len_off + 8].try_into().unwrap()) as usize;
            let user_ptr = u64::from_le_bytes(payload[ptr_off..ptr_off + 8].try_into().unwrap());
            if append_aux(&mut payload, &mut aux, ptr_off, user_ptr, len).is_err() {
                return IoctlAction::Complete(E2BIG);
            }
        }
        flags |= wire::FLAG_VERSION_AUX;
    } else if resolved.name == "AMDGPU_BO_LIST" {
        let count = u32::from_le_bytes(payload[8..12].try_into().unwrap()) as usize;
        let entry_size = u32::from_le_bytes(payload[12..16].try_into().unwrap()) as usize;
        let user_ptr = u64::from_le_bytes(payload[16..24].try_into().unwrap());
        if count > 8192 || (count != 0 && entry_size != 8) {
            return IoctlAction::Complete(EINVAL);
        }
        let Some(bytes) = count.checked_mul(entry_size) else {
            return IoctlAction::Complete(E2BIG);
        };
        if append_aux_from_user(&mut payload, &mut aux, 16, user_ptr, bytes).is_err() {
            return IoctlAction::Complete(EFAULT);
        }
        flags |= wire::FLAG_BO_LIST_AUX;
    } else if resolved.name == "AMDGPU_CS" {
        let count = u32::from_le_bytes(payload[8..12].try_into().unwrap()) as usize;
        let chunks_ptr = u64::from_le_bytes(payload[16..24].try_into().unwrap());
        if count == 0 || count > 64 {
            return IoctlAction::Complete(EINVAL);
        }
        let pointer_bytes = match crate::uaccess::copy_from_user(chunks_ptr, count * 8) {
            Ok(bytes) => bytes,
            Err(_) => return IoctlAction::Complete(EFAULT),
        };
        let pointer_array_offset = payload.len();
        payload.extend_from_slice(&pointer_bytes);
        let headers_offset = payload.len();
        let Some(headers_end) = headers_offset.checked_add(count * 16) else {
            return IoctlAction::Complete(E2BIG);
        };
        if headers_end > wire::MAX_PAYLOAD {
            return IoctlAction::Complete(E2BIG);
        }
        payload.resize(headers_end, 0);
        let mut chunk_sources = Vec::with_capacity(count);
        for index in 0..count {
            let source =
                u64::from_le_bytes(pointer_bytes[index * 8..index * 8 + 8].try_into().unwrap());
            let header = match crate::uaccess::copy_from_user(source, 16) {
                Ok(bytes) => bytes,
                Err(_) => return IoctlAction::Complete(EFAULT),
            };
            payload[headers_offset + index * 16..headers_offset + (index + 1) * 16]
                .copy_from_slice(&header);
            chunk_sources.push(u64::from_le_bytes(header[8..16].try_into().unwrap()));
        }
        for (index, source) in chunk_sources.into_iter().enumerate() {
            let header = headers_offset + index * 16;
            let dwords = u32::from_le_bytes(payload[header + 4..header + 8].try_into().unwrap());
            let Some(bytes) = (dwords as usize).checked_mul(4) else {
                return IoctlAction::Complete(E2BIG);
            };
            let Some(end) = payload.len().checked_add(bytes) else {
                return IoctlAction::Complete(E2BIG);
            };
            if end > wire::MAX_PAYLOAD {
                return IoctlAction::Complete(E2BIG);
            }
            let data = match crate::uaccess::copy_from_user(source, bytes) {
                Ok(bytes) => bytes,
                Err(_) => return IoctlAction::Complete(EFAULT),
            };
            payload.extend_from_slice(&data);
        }
        // The daemon reconstructs both levels of pointers from this canonical
        // layout; no raw client pointer is consumed there.
        let _ = pointer_array_offset;
        flags |= wire::FLAG_CS_AUX;
    }
    let request_id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
    let pending = Pending {
        request_id,
        user_arg,
        arg_len,
        copy_flat_out: matches!(
            resolved.copy,
            ath_render_broker::CopyDir::Out | ath_render_broker::CopyDir::InOut
        ),
        aux,
    };
    let request = Request {
        header: wire::RequestHeader {
            version: wire::VERSION,
            op: wire::OP_IOCTL,
            request_id,
            client_id,
            ioctl_cmd: cmd,
            flags,
            arg_len: arg_len as u32,
            payload_len: payload.len() as u32,
        },
        payload,
    };
    IoctlAction::BlockNew {
        request_id,
        request: PreparedIoctl {
            task_id,
            pending,
            request,
        },
    }
}

/// Called inside `block_current_task_with`, after the task is in switch_stash.
/// A daemon completion can now safely wake either the stash or blocked queue.
pub fn enqueue_ioctl(prepared: PreparedIoctl) {
    let mut broker = BROKER.lock();
    if broker.service_task.is_none() {
        return;
    }
    broker.waiting.insert(prepared.task_id, prepared.pending);
    broker.queue.push_back(prepared.request);
}

/// Close the retry/reblock race for a task that was spuriously resumed: called
/// only after the task has entered the scheduler switch stash.
pub fn wake_if_response(task_id: u64, request_id: u64) {
    let ready = BROKER.lock().responses.contains_key(&request_id);
    if ready {
        crate::scheduler::wake_thread(crate::task::TaskId::from_raw(task_id));
    }
}

fn finish_response(pending: Pending, mut response: Response) -> i64 {
    if response.status != 0 {
        return response.status as i64;
    }
    let required = pending.aux.iter().fold(pending.arg_len, |end, aux| {
        end.max(aux.payload_offset.saturating_add(aux.len))
    });
    if response.payload.len() < required {
        return EINVAL;
    }
    for aux in pending.aux {
        response.payload[aux.pointer_field..aux.pointer_field + 8]
            .copy_from_slice(&aux.user_ptr.to_le_bytes());
        if aux.copy_out
            && aux.len != 0
            && crate::uaccess::copy_to_user(
                aux.user_ptr,
                &response.payload[aux.payload_offset..aux.payload_offset + aux.len],
            )
            .is_err()
        {
            return EFAULT;
        }
    }
    if pending.copy_flat_out
        && crate::uaccess::copy_to_user(pending.user_arg, &response.payload[..pending.arg_len])
            .is_err()
    {
        return EFAULT;
    }
    0
}

fn append_aux(
    payload: &mut Vec<u8>,
    aux: &mut Vec<AuxCopy>,
    pointer_field: usize,
    user_ptr: u64,
    len: usize,
) -> Result<(), ()> {
    let offset = payload.len();
    let end = offset.checked_add(len).ok_or(())?;
    if end > wire::MAX_PAYLOAD || (len != 0 && user_ptr == 0) {
        return Err(());
    }
    payload.resize(end, 0);
    aux.push(AuxCopy {
        pointer_field,
        user_ptr,
        payload_offset: offset,
        len,
        copy_out: true,
    });
    Ok(())
}

fn append_aux_from_user(
    payload: &mut Vec<u8>,
    aux: &mut Vec<AuxCopy>,
    pointer_field: usize,
    user_ptr: u64,
    len: usize,
) -> Result<(), ()> {
    let offset = payload.len();
    let end = offset.checked_add(len).ok_or(())?;
    if end > wire::MAX_PAYLOAD || (len != 0 && user_ptr == 0) {
        return Err(());
    }
    let bytes = crate::uaccess::copy_from_user(user_ptr, len)?;
    payload.extend_from_slice(&bytes);
    aux.push(AuxCopy {
        pointer_field,
        user_ptr,
        payload_offset: offset,
        len,
        copy_out: false,
    });
    Ok(())
}
