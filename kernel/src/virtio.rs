extern crate alloc;

use crate::memory::GlobalFrameAllocator;
use core::sync::atomic::AtomicU16;
use spin::Mutex;
use x86_64::instructions::port::{Port, PortReadOnly};
use x86_64::structures::paging::FrameAllocator;

const VIRTIO_BLK_T_IN: u32 = 0;
const VIRTIO_BLK_T_OUT: u32 = 1;

/// Upper bound (TSC cycles) on a virtio-blk completion busy-poll before giving up
/// with an I/O error instead of spinning forever. The early-boot RaeFS smoketest
/// intermittently never sees a completion (lost notification / used-ring race),
/// which previously froze the whole boot. ~10e9 cycles is several seconds — far
/// beyond a real completion (microseconds) yet bounded so boot always proceeds.
const VIRTIO_POLL_DEADLINE_CYCLES: u64 = 10_000_000_000;

const VRING_DESC_F_NEXT: u16 = 1;
const VRING_DESC_F_WRITE: u16 = 2;

#[repr(C, align(16))]
#[derive(Debug, Clone, Copy)]
pub struct VirtqDesc {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

#[repr(C)]
#[derive(Debug)]
pub struct VirtqAvail {
    pub flags: u16,
    pub idx: AtomicU16,
    // Variable size, we will allocate enough space for 256 entries
    pub ring: [u16; 256],
    pub used_event: u16,
}

#[repr(C, align(4))]
#[derive(Debug, Clone, Copy)]
pub struct VirtqUsedElem {
    pub id: u32,
    pub len: u32,
}

#[repr(C, align(4))]
#[derive(Debug)]
pub struct VirtqUsed {
    pub flags: u16,
    pub idx: AtomicU16,
    pub ring: [VirtqUsedElem; 256],
    pub avail_event: u16,
}

#[repr(C)]
pub struct VirtioBlkReq {
    pub type_: u32,
    pub reserved: u32,
    pub sector: u64,
}

pub struct VirtQueue {
    queue_size: u16,
    desc_table: *mut VirtqDesc,
    avail_ring: *mut VirtqAvail,
    used_ring: *mut VirtqUsed,
    last_used_idx: u16,
    free_head: u16,
    num_free: u16,
    port_base: u16,
    queue_index: u16,
    pub completed_requests: [bool; 256],
}

unsafe impl Send for VirtQueue {}
unsafe impl Sync for VirtQueue {}

impl VirtQueue {
    pub fn new(port_base: u16, queue_index: u16) -> Option<Self> {
        let mut queue_select: Port<u16> = Port::new(port_base + 0x0E);
        let mut queue_size_port: PortReadOnly<u16> = PortReadOnly::new(port_base + 0x0C);
        let mut queue_addr: Port<u32> = Port::new(port_base + 0x08);

        unsafe {
            queue_select.write(queue_index);
            let queue_size = queue_size_port.read();
            if queue_size == 0 {
                crate::serial_println!("[virtio] Queue {} is unavailable", queue_index);
                return None;
            }
            if queue_size > 256 {
                crate::serial_println!(
                    "[virtio] Queue {} size {} too large (max 256 supported)",
                    queue_index,
                    queue_size
                );
                return None;
            }

            // Allocate physically contiguous memory for the queue.
            // For Queue Size = 256:
            // Desc: 16 * 256 = 4096 bytes (1 page)
            // Avail: 6 + 2 * 256 = 518 bytes. Pad to page boundary -> 4096 bytes (1 page)
            // Used: 6 + 8 * 256 = 2054 bytes -> 1 page.
            // Total = 3 contiguous pages.
            // The Virtio spec allows Avail to be in the same page as Desc if it fits, but aligning is easier.
            // The formula dictates: desc + avail -> pad to 4096 -> used.
            // size = align_up(16 * size + 6 + 2 * size, 4096) + align_up(6 + 8 * size, 4096)
            // For 256: align_up(4096 + 518, 4096) + 4096 = 8192 + 4096 = 12288 (3 pages).

            // BUG-16: allocate_contiguous_frames takes a buddy ORDER (2^order
            // pages), not a page count. The layout above needs 3 contiguous
            // pages, so order 2 = 4 pages covers it; order 3 = 8 pages wasted 5.
            let phys_base = crate::memory::allocate_contiguous_frames(2)
                .expect("virtio: OOM for queue")
                .as_u64();
            let virt_addr = phys_base + crate::memory::PHYS_MEM_OFFSET.get().unwrap().as_u64();
            core::ptr::write_bytes(virt_addr as *mut u8, 0, 3 * 4096);

            let desc_table = virt_addr as *mut VirtqDesc;
            let avail_ring = (virt_addr + 4096u64) as *mut VirtqAvail;
            let used_ring = (virt_addr + 8192u64) as *mut VirtqUsed;

            // Link free descriptors
            for i in 0..queue_size - 1 {
                (*desc_table.add(i as usize)).next = i + 1;
            }
            (*desc_table.add((queue_size - 1) as usize)).next = 0;

            // Pass page number to device
            queue_addr.write((phys_base / 4096) as u32);

            Some(VirtQueue {
                queue_size,
                desc_table,
                avail_ring,
                used_ring,
                last_used_idx: 0,
                free_head: 0,
                num_free: queue_size,
                port_base,
                queue_index,
                completed_requests: [false; 256],
            })
        }
    }

    pub fn notify(&self) {
        let mut queue_notify: Port<u16> = Port::new(self.port_base + 0x10);
        unsafe {
            queue_notify.write(self.queue_index);
        }
    }

    pub fn submit_request(
        &mut self,
        req_phys: u64,
        buf_phys: u64,
        buf_len: u32,
        is_write: bool,
        status_phys: u64,
    ) -> Option<u16> {
        if self.num_free < 3 {
            crate::serial_println!("[virtio] Not enough free descriptors");
            return None;
        }

        let desc0 = self.free_head;
        let desc1 = unsafe { (*self.desc_table.add(desc0 as usize)).next };
        let desc2 = unsafe { (*self.desc_table.add(desc1 as usize)).next };
        self.free_head = unsafe { (*self.desc_table.add(desc2 as usize)).next };
        self.num_free -= 3;

        unsafe {
            (*self.desc_table.add(desc0 as usize)).addr = req_phys;
            (*self.desc_table.add(desc0 as usize)).len = 16;
            (*self.desc_table.add(desc0 as usize)).flags = VRING_DESC_F_NEXT;
            (*self.desc_table.add(desc0 as usize)).next = desc1;

            (*self.desc_table.add(desc1 as usize)).addr = buf_phys;
            (*self.desc_table.add(desc1 as usize)).len = buf_len;
            (*self.desc_table.add(desc1 as usize)).flags =
                VRING_DESC_F_NEXT | if is_write { 0 } else { VRING_DESC_F_WRITE };
            (*self.desc_table.add(desc1 as usize)).next = desc2;

            (*self.desc_table.add(desc2 as usize)).addr = status_phys;
            (*self.desc_table.add(desc2 as usize)).len = 1;
            (*self.desc_table.add(desc2 as usize)).flags = VRING_DESC_F_WRITE;
        }

        unsafe {
            let avail_idx = (*self.avail_ring)
                .idx
                .load(core::sync::atomic::Ordering::Relaxed);
            (*self.avail_ring).ring[(avail_idx % self.queue_size) as usize] = desc0;

            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);

            (*self.avail_ring).idx.store(
                avail_idx.wrapping_add(1),
                core::sync::atomic::Ordering::Release,
            );
        }

        self.completed_requests[desc0 as usize] = false;

        // (Per-request debug print removed — it fired on EVERY block request and
        // flooded the serial path; under KVM the fast CPU then blocks on slow
        // UART+framebuffer output, pushing boot past the CI timeout. Virtio
        // status is already visible via the smoketest + /proc. Re-add behind a
        // debug gate if a specific virtio regression needs per-request tracing.)
        self.notify();
        Some(desc0)
    }

    pub fn process_used_ring(&mut self) {
        let used_idx = unsafe {
            (*self.used_ring)
                .idx
                .load(core::sync::atomic::Ordering::Acquire)
        };
        while self.last_used_idx != used_idx {
            let used_elem =
                unsafe { (*self.used_ring).ring[(self.last_used_idx % self.queue_size) as usize] };
            self.last_used_idx = self.last_used_idx.wrapping_add(1);

            let head = used_elem.id as u16;
            // (Per-completion debug print removed — same per-request flood as
            // submit_request above; it dominated boot serial output under KVM.)
            unsafe {
                let d1 = (*self.desc_table.add(head as usize)).next;
                let d2 = (*self.desc_table.add(d1 as usize)).next;
                (*self.desc_table.add(d2 as usize)).next = self.free_head;
                self.free_head = head;
                self.num_free += 3;
            }

            self.completed_requests[head as usize] = true;
            crate::scheduler::unblock_virtio_waiters(head);
        }
    }
}

/// A single physically-contiguous, page-aligned DMA bounce region.
///
/// Per-task kernel stacks are heap-backed (`alloc_kernel_stack`), and the
/// kernel heap is mapped page-by-page with **non-contiguous** physical frames
/// (`memory::allocator::init_heap`). A DMA buffer placed directly on such a
/// stack can straddle a 4 KiB page boundary whose two halves are physically
/// non-contiguous; the device, given only the start physical address + length,
/// then writes the tail into the wrong physical frame and corrupts unrelated
/// kernel memory. We bounce every request through this single contiguous page
/// so no descriptor ever spans a physical discontinuity.
struct VirtioDma {
    phys: u64,
    virt: u64,
}

unsafe impl Send for VirtioDma {}

impl VirtioDma {
    // Sub-region offsets within the 4 KiB DMA page. All three regions live in
    // one physically-contiguous frame, so none can straddle a page boundary.
    const REQ_OFF: u64 = 0; // VirtioBlkReq (16 bytes)
    const STATUS_OFF: u64 = 16; // status byte (1 byte)
    const DATA_OFF: u64 = 64; // payload (up to MAX_DATA bytes)
    const MAX_DATA: usize = 4096 - 64;

    fn alloc() -> Option<VirtioDma> {
        let phys = crate::memory::allocate_contiguous_frames(1)?.as_u64();
        let offset = *crate::memory::PHYS_MEM_OFFSET.get()?;
        let virt = (offset + phys).as_u64();
        unsafe {
            core::ptr::write_bytes(virt as *mut u8, 0, 4096);
        }
        Some(VirtioDma { phys, virt })
    }
}

pub struct VirtioBlk {
    pub port_base: u16,
    pub queue: Mutex<VirtQueue>,
    dma: Mutex<VirtioDma>,
}

impl VirtioBlk {
    /// VirtIO DMA requires physical addresses; always walk the kernel PML4 so
    /// per-task kernel stacks remain valid while user CR3 is loaded.
    #[allow(dead_code)]
    fn virt_to_phys(virt: crate::arch::VirtAddr) -> Result<u64, ()> {
        crate::memory::kernel_translate_addr(virt)
            .map(|p| p.as_u64())
            .ok_or(())
    }

    pub fn read_block(&self, sector: u64, buf: &mut [u8]) -> Result<(), ()> {
        if buf.len() > VirtioDma::MAX_DATA {
            return Err(());
        }

        // Hold the DMA region for the entire request: req/status/data all live
        // in one contiguous page so no descriptor can straddle a physical
        // discontinuity in the heap-backed kernel stack.
        let dma = self.dma.lock();
        let req_virt = (dma.virt + VirtioDma::REQ_OFF) as *mut VirtioBlkReq;
        let status_virt = (dma.virt + VirtioDma::STATUS_OFF) as *mut u8;
        let data_virt = (dma.virt + VirtioDma::DATA_OFF) as *mut u8;
        unsafe {
            core::ptr::write_volatile(
                req_virt,
                VirtioBlkReq {
                    type_: VIRTIO_BLK_T_IN,
                    reserved: 0,
                    sector,
                },
            );
            core::ptr::write_volatile(status_virt, 255u8);
        }

        let req_phys = dma.phys + VirtioDma::REQ_OFF;
        let buf_phys = dma.phys + VirtioDma::DATA_OFF;
        let status_phys = dma.phys + VirtioDma::STATUS_OFF;

        let head = {
            let mut q = self.queue.lock();
            q.submit_request(req_phys, buf_phys, buf.len() as u32, false, status_phys)
                .ok_or(())?
        };

        let poll_start = unsafe { core::arch::x86_64::_rdtsc() };
        loop {
            let completed = x86_64::instructions::interrupts::without_interrupts(|| {
                let mut q = self.queue.lock();

                // If it's already completed in the interrupt handler, great!
                if q.completed_requests[head as usize] {
                    return true;
                }

                // If the scheduler isn't active yet, we must manually process the ring and spin.
                if !crate::scheduler::BOOT_COMPLETE.load(core::sync::atomic::Ordering::Relaxed) {
                    q.process_used_ring();
                    if q.completed_requests[head as usize] {
                        return true;
                    }
                    drop(q);
                    core::hint::spin_loop();
                    return false;
                }

                // Normal async block
                crate::scheduler::block_current_task_with(
                    crate::task::TaskState::BlockedOnVirtio(head),
                    || drop(q), // Important: drop lock inside the blocking callback so we are safely in `blocked_tasks` before the interrupt handler can process the ring!
                );
                false
            });
            if completed {
                break;
            }
            if unsafe { core::arch::x86_64::_rdtsc() }.wrapping_sub(poll_start)
                > VIRTIO_POLL_DEADLINE_CYCLES
            {
                crate::serial_println!(
                    "[virtio] read_block timeout (sector={}) — returning Err",
                    sector
                );
                return Err(());
            }
        }

        if !crate::scheduler::BOOT_COMPLETE.load(core::sync::atomic::Ordering::Relaxed) {
            let mut isr_port: x86_64::instructions::port::PortReadOnly<u8> =
                x86_64::instructions::port::PortReadOnly::new(self.port_base + 0x13);
            unsafe { isr_port.read() };
        }

        let status = unsafe { core::ptr::read_volatile(status_virt) };
        if status == 0 {
            // Copy the device-written payload out of the bounce page.
            unsafe {
                core::ptr::copy_nonoverlapping(data_virt, buf.as_mut_ptr(), buf.len());
            }
            Ok(())
        } else {
            Err(())
        }
    }

    pub fn write_block(&self, sector: u64, buf: &[u8]) -> Result<(), ()> {
        if buf.len() > VirtioDma::MAX_DATA {
            return Err(());
        }

        let dma = self.dma.lock();
        let req_virt = (dma.virt + VirtioDma::REQ_OFF) as *mut VirtioBlkReq;
        let status_virt = (dma.virt + VirtioDma::STATUS_OFF) as *mut u8;
        let data_virt = (dma.virt + VirtioDma::DATA_OFF) as *mut u8;
        unsafe {
            core::ptr::write_volatile(
                req_virt,
                VirtioBlkReq {
                    type_: VIRTIO_BLK_T_OUT,
                    reserved: 0,
                    sector,
                },
            );
            core::ptr::write_volatile(status_virt, 255u8);
            // Stage the caller's payload into the contiguous bounce page.
            core::ptr::copy_nonoverlapping(buf.as_ptr(), data_virt, buf.len());
        }

        let req_phys = dma.phys + VirtioDma::REQ_OFF;
        let buf_phys = dma.phys + VirtioDma::DATA_OFF;
        let status_phys = dma.phys + VirtioDma::STATUS_OFF;

        let head = {
            let mut q = self.queue.lock();
            q.submit_request(req_phys, buf_phys, buf.len() as u32, true, status_phys)
                .ok_or(())?
        };

        let poll_start = unsafe { core::arch::x86_64::_rdtsc() };
        loop {
            let completed = x86_64::instructions::interrupts::without_interrupts(|| {
                let mut q = self.queue.lock();

                if q.completed_requests[head as usize] {
                    return true;
                }

                if !crate::scheduler::BOOT_COMPLETE.load(core::sync::atomic::Ordering::Relaxed) {
                    q.process_used_ring();
                    if q.completed_requests[head as usize] {
                        return true;
                    }
                    drop(q);
                    core::hint::spin_loop();
                    return false;
                }

                // Normal async block
                crate::scheduler::block_current_task_with(
                    crate::task::TaskState::BlockedOnVirtio(head),
                    || drop(q), // Important: drop lock inside the blocking callback so we are safely in `blocked_tasks` before the interrupt handler can process the ring!
                );
                false
            });
            if completed {
                break;
            }
            if unsafe { core::arch::x86_64::_rdtsc() }.wrapping_sub(poll_start)
                > VIRTIO_POLL_DEADLINE_CYCLES
            {
                crate::serial_println!(
                    "[virtio] write_block timeout (sector={}) — returning Err",
                    sector
                );
                return Err(());
            }
        }

        if !crate::scheduler::BOOT_COMPLETE.load(core::sync::atomic::Ordering::Relaxed) {
            let mut isr_port: x86_64::instructions::port::PortReadOnly<u8> =
                x86_64::instructions::port::PortReadOnly::new(self.port_base + 0x13);
            unsafe { isr_port.read() };
        }

        let status = unsafe { core::ptr::read_volatile(status_virt) };
        if status == 0 {
            Ok(())
        } else {
            Err(())
        }
    }
}

impl crate::block_io::BlockDevice for VirtioBlk {
    fn read_sector(&self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        self.read_block(lba, buf)
            .map_err(|_| "virtio-blk read failed")
    }

    fn write_sector(&self, lba: u64, buf: &[u8]) -> Result<(), &'static str> {
        crate::block_io::safe_mode_guard_write(lba, buf.len(), "virtio-blk")?;
        self.write_block(lba, buf)
            .map_err(|_| "virtio-blk write failed")
    }

    fn sector_size(&self) -> usize {
        512
    }

    fn total_sectors(&self) -> u64 {
        // BUG-20: read the real capacity from config space (offset 0x14/0x18,
        // the 64-bit sector count) instead of returning 0, so a caller checking
        // capacity on the inner VirtioBlk (not just the VirtioBlockDevice
        // wrapper) doesn't think the disk is empty.
        unsafe {
            let mut lo: PortReadOnly<u32> = PortReadOnly::new(self.port_base + 0x14);
            let mut hi: PortReadOnly<u32> = PortReadOnly::new(self.port_base + 0x18);
            ((hi.read() as u64) << 32) | lo.read() as u64
        }
    }
}

/// Safety: VirtioBlk's internal state is protected by a Mutex.
unsafe impl Send for VirtioBlk {}

pub static VIRTIO_BLK: spin::Once<VirtioBlk> = spin::Once::new();

pub struct VirtioBlockDevice;

impl crate::block_io::BlockDevice for VirtioBlockDevice {
    fn read_sector(&self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        let blk = VIRTIO_BLK.get().ok_or("virtio-blk not initialized")?;
        blk.read_block(lba, buf).map_err(|_| "virtio read failed")
    }

    fn write_sector(&self, lba: u64, buf: &[u8]) -> Result<(), &'static str> {
        crate::block_io::safe_mode_guard_write(lba, buf.len(), "virtio-blk-dev")?;
        let blk = VIRTIO_BLK.get().ok_or("virtio-blk not initialized")?;
        blk.write_block(lba, buf).map_err(|_| "virtio write failed")
    }

    fn sector_size(&self) -> usize {
        512
    }

    fn total_sectors(&self) -> u64 {
        let blk = VIRTIO_BLK.get();
        match blk {
            Some(b) => unsafe {
                let mut lo: PortReadOnly<u32> = PortReadOnly::new(b.port_base + 0x14);
                let mut hi: PortReadOnly<u32> = PortReadOnly::new(b.port_base + 0x18);
                (hi.read() as u64) << 32 | lo.read() as u64
            },
            None => 0,
        }
    }
}

unsafe impl Send for VirtioBlockDevice {}

pub fn init(device: &crate::pci::PciDevice) {
    if device.vendor_id != 0x1AF4 || device.device_id != 0x1001 {
        return; // Not a virtio-blk device
    }

    let port_base = (device.bars[0] & !1) as u16;
    crate::serial_println!(
        "[virtio] Initializing virtio-blk at I/O port {:#06x}",
        port_base
    );

    unsafe {
        let mut status_port: Port<u8> = Port::new(port_base + 0x12);

        // 1. Reset device
        status_port.write(0);

        // 2. Set ACKNOWLEDGE
        let mut status = status_port.read();
        status |= 1;
        status_port.write(status);

        // 3. Set DRIVER
        status |= 2;
        status_port.write(status);

        // 4. Negotiate Features (skip for now, accept defaults)

        // 5. Setup queue 0
        let queue = match VirtQueue::new(port_base, 0) {
            Some(q) => q,
            None => {
                crate::serial_println!("[virtio] Failed to setup queue 0");
                status_port.write(status | 128); // FAILED
                return;
            }
        };

        // 5.5. Route IRQ
        crate::apic::route_irq(
            device.irq_line as u32,
            crate::interrupts::InterruptIndex::VirtioBlk.as_u8(),
        );

        // 6. Set DRIVER_OK
        status |= 4;
        status_port.write(status);

        crate::serial_println!("[virtio] virtio-blk initialized successfully");
        let dma = match VirtioDma::alloc() {
            Some(d) => d,
            None => {
                crate::serial_println!("[virtio] Failed to allocate DMA bounce page");
                status_port.write(status | 128); // FAILED
                return;
            }
        };

        let blk = VirtioBlk {
            port_base,
            queue: Mutex::new(queue),
            dma: Mutex::new(dma),
        };

        VIRTIO_BLK.call_once(|| blk);

        crate::block_io::set_active_block_device(alloc::boxed::Box::new(VirtioBlockDevice));

        // TEST: Read sector 0
        let mut buf = [0u8; 512];
        match VIRTIO_BLK.get().unwrap().read_block(0, &mut buf) {
            Ok(_) => {
                if let Ok(s) = core::str::from_utf8(&buf[0..32]) {
                    crate::serial_println!("[virtio] Block 0 read test: {:?}", s);
                } else {
                    crate::serial_println!("[virtio] Block 0 read test raw: {:?}", &buf[0..32]);
                }
            }
            Err(_) => {
                crate::serial_println!("[virtio] Failed to read Block 0");
            }
        }
    }
}
