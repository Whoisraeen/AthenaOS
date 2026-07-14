// kernel/src/virtio_net.rs
//
// virtio-net legacy I/O-port driver — sends and receives Ethernet
// frames against a `virtio-net-pci` device. Year-1 networking
// deliverable: this is what unblocks ARP/DHCP/ping via smoltcp.
//
// Concept §Networking: AthenaOS uses AthNet on real hardware. virtio-net
// is the guest-only convenience driver for QEMU/KVM that lets us run
// the network stack without writing an e1000/realtek/Intel-igb driver.
//
// Protocol: virtio 0.9.5 legacy PCI transport (port I/O at BAR0).
//   • Status register (port_base + 0x12): ACKNOWLEDGE | DRIVER |
//     FEATURES_OK | DRIVER_OK.
//   • Feature bits: only VIRTIO_NET_F_MAC (5) accepted. CSUM/GSO/TSO/
//     mergeable-rxbuf are *not* negotiated — keeps the rx path simple
//     (each rx buffer is exactly one frame).
//   • Two virtqueues: 0 = receiveq, 1 = transmitq.
//   • Every TX/RX descriptor carries a 12-byte virtio_net_hdr prefix.

#![allow(dead_code)]

use crate::memory::GlobalFrameAllocator;
use core::sync::atomic::{fence, AtomicU16, AtomicU32, Ordering};
use spin::{Mutex, Once};
use x86_64::instructions::port::{Port, PortReadOnly};
use x86_64::structures::paging::FrameAllocator;

// ── Constants ──────────────────────────────────────────────────────────

const VIRTIO_STATUS_ACKNOWLEDGE: u8 = 1;
const VIRTIO_STATUS_DRIVER: u8 = 2;
const VIRTIO_STATUS_DRIVER_OK: u8 = 4;
const VIRTIO_STATUS_FEATURES_OK: u8 = 8;

const VIRTIO_NET_F_MAC: u32 = 1 << 5;

const VRING_DESC_F_NEXT: u16 = 1;
const VRING_DESC_F_WRITE: u16 = 2;

// The QSIZE constant must match the device's queue_size exactly
// — legacy virtio uses queue_size for ring indexing both sides. We
// allocate AvailRing/UsedRing sized to QSIZE; if it's smaller than
// the device's queue_size, the device reads/writes past our buffers
// (no RX would ever arrive). QEMU virtio-net advertises 256 by
// default; everything below scales to that.
const QSIZE: u16 = 256;
// virtio_net_hdr is 10 bytes WITHOUT VIRTIO_NET_F_MRG_RXBUF, 12 bytes
// WITH it. We negotiate only VIRTIO_NET_F_MAC, so legacy 10-byte header.
// The previous 12-byte assumption was wrong: TX prefixed 2 extra zero
// bytes which QEMU saw as the start of the Ethernet dst_mac (becoming
// 0x00,0x00,...) and silently dropped the frame as malformed. RX
// mis-parsed frames the same way, missing two bytes at the start.
const NET_HDR_LEN: usize = 10;
const RX_BUF_LEN: usize = 1526; // 1500 MTU + headers + headroom

// ── Virtqueue layout ───────────────────────────────────────────────────

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct Desc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

#[repr(C, align(2))]
struct AvailRing {
    flags: u16,
    idx: AtomicU16,
    ring: [u16; QSIZE as usize],
    used_event: u16,
}

#[repr(C, align(4))]
#[derive(Clone, Copy)]
struct UsedElem {
    id: u32,
    len: u32,
}

#[repr(C, align(4))]
struct UsedRing {
    flags: u16,
    idx: AtomicU16,
    ring: [UsedElem; QSIZE as usize],
    avail_event: u16,
}

struct VirtQueue {
    desc: *mut Desc,
    avail: *mut AvailRing,
    used: *mut UsedRing,
    last_used: u16,
    /// For RX: phys/virt of each backing buffer keyed by descriptor index.
    buf_phys: [u64; QSIZE as usize],
    buf_virt: [u64; QSIZE as usize],
    queue_index: u16,
    /// Physical page number passed to legacy queue_addr (for dumps).
    queue_pfn: u32,
}

/// RX bring-up counters — distinguish "used ring advanced" vs "delivered".
#[derive(Default)]
pub struct RxDiagStats {
    pub polls: AtomicU32,
    pub isr_reads: AtomicU32,
    pub isr_used_ring: AtomicU32,
    pub used_advanced: AtomicU32,
    pub delivered: AtomicU32,
    pub skip_len: AtomicU32,
    pub skip_desc: AtomicU32,
}

pub static RX_DIAG: RxDiagStats = RxDiagStats {
    polls: AtomicU32::new(0),
    isr_reads: AtomicU32::new(0),
    isr_used_ring: AtomicU32::new(0),
    used_advanced: AtomicU32::new(0),
    delivered: AtomicU32::new(0),
    skip_len: AtomicU32::new(0),
    skip_desc: AtomicU32::new(0),
};

unsafe impl Send for VirtQueue {}
unsafe impl Sync for VirtQueue {}

// ── Driver state ───────────────────────────────────────────────────────

pub struct VirtioNet {
    port_base: u16,
    irq_line: u8,
    mac: [u8; 6],
    rx: Mutex<VirtQueue>,
    tx: Mutex<VirtQueue>,
    tx_done: spin::Mutex<usize>, // running count of successful TX
    rx_done: spin::Mutex<usize>, // running count of received frames
}

impl VirtioNet {
    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }

    /// Read legacy PCI ISR (offset 0x13). Required to clear virtio IRQ latch.
    pub fn ack_isr(&self) -> u8 {
        let mut isr_port: PortReadOnly<u8> = PortReadOnly::new(self.port_base + 0x13);
        let status = unsafe { isr_port.read() };
        RX_DIAG.isr_reads.fetch_add(1, Ordering::Relaxed);
        if (status & 0x01) != 0 {
            RX_DIAG.isr_used_ring.fetch_add(1, Ordering::Relaxed);
        }
        status
    }

    /// Log virtqueue indices for bring-up diagnosis.
    pub fn dump_queue_state(&self, tag: &str) {
        let rx = self.rx.lock();
        let tx = self.tx.lock();
        let (rx_avail, rx_used, tx_avail, tx_used) = unsafe {
            (
                (*rx.avail).idx.load(Ordering::Acquire),
                (*rx.used).idx.load(Ordering::Acquire),
                (*tx.avail).idx.load(Ordering::Acquire),
                (*tx.used).idx.load(Ordering::Acquire),
            )
        };
        crate::serial_println!(
            "[virtio-net] {}: rx avail.idx={} used.idx={} last_used={} pending={} \
             tx avail.idx={} used.idx={} last_used={} isr_status={:#x}",
            tag,
            rx_avail,
            rx_used,
            rx.last_used,
            rx_used.wrapping_sub(rx.last_used),
            tx_avail,
            tx_used,
            tx.last_used,
            self.ack_isr(),
        );
        crate::serial_println!(
            "[virtio-net] {}: diag polls={} used_adv={} delivered={} skip_len={} skip_desc={} rx_done={}",
            tag,
            RX_DIAG.polls.load(Ordering::Relaxed),
            RX_DIAG.used_advanced.load(Ordering::Relaxed),
            RX_DIAG.delivered.load(Ordering::Relaxed),
            RX_DIAG.skip_len.load(Ordering::Relaxed),
            RX_DIAG.skip_desc.load(Ordering::Relaxed),
            self.rx_count(),
        );
    }

    /// Transmit one Ethernet frame. Returns Ok on submission (device
    /// has accepted the descriptor); device confirms via used ring.
    pub fn tx_frame(&self, frame: &[u8]) -> Result<(), &'static str> {
        if frame.len() > 1514 {
            return Err("virtio-net: frame too large");
        }
        let mut tx = self.tx.lock();

        // We reuse descriptor 0 for every TX (single in-flight). Once the
        // used ring confirms it, we proceed. For the boot smoketest this
        // is enough; real workloads need a ring of descriptors.
        let virt = tx.buf_virt[0] as *mut u8;
        unsafe {
            // virtio_net_hdr — zero everything.
            core::ptr::write_bytes(virt, 0, NET_HDR_LEN);
            // Then the Ethernet frame.
            core::ptr::copy_nonoverlapping(frame.as_ptr(), virt.add(NET_HDR_LEN), frame.len());
            let d = &mut *tx.desc.add(0);
            d.addr = tx.buf_phys[0];
            d.len = (NET_HDR_LEN + frame.len()) as u32;
            d.flags = 0;
            d.next = 0;
        }
        unsafe {
            let avail_idx = (*tx.avail).idx.load(Ordering::Relaxed);
            (*tx.avail).ring[(avail_idx % QSIZE) as usize] = 0;
            fence(Ordering::SeqCst);
            (*tx.avail)
                .idx
                .store(avail_idx.wrapping_add(1), Ordering::Release);
        }
        // Notify device.
        let mut notify: Port<u16> = Port::new(self.port_base + 0x10);
        unsafe {
            notify.write(1);
        }

        // Wait for the used ring to catch up. QEMU completes within
        // microseconds; the cap exists so a misbehaving device can't
        // hang the caller.
        for _ in 0..50_000 {
            let used_idx = unsafe { (*tx.used).idx.load(Ordering::Acquire) };
            if used_idx != tx.last_used {
                tx.last_used = used_idx;
                *self.tx_done.lock() += 1;
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err("virtio-net: TX completion timeout")
    }

    /// Poll the RX virtqueue for received frames and pass each one to
    /// `cb`. Returns the number of frames delivered.
    pub fn rx_poll<F: FnMut(&[u8])>(&self, mut cb: F) -> usize {
        RX_DIAG.polls.fetch_add(1, Ordering::Relaxed);
        let _isr = self.ack_isr();

        let mut rx = self.rx.lock();
        let mut delivered = 0usize;
        let mut log_budget = 8usize;
        loop {
            let used_idx = unsafe { (*rx.used).idx.load(Ordering::Acquire) };
            if rx.last_used == used_idx {
                break;
            }
            RX_DIAG.used_advanced.fetch_add(1, Ordering::Relaxed);

            let slot = (rx.last_used % QSIZE) as usize;
            let elem = unsafe { (*rx.used).ring[slot] };
            let desc_id = elem.id as usize;
            let total_len = elem.len as usize;

            if log_budget > 0 {
                crate::serial_println!(
                    "[virtio-net] rx used slot={} id={} len={} (hdr={})",
                    slot,
                    desc_id,
                    total_len,
                    NET_HDR_LEN,
                );
                log_budget -= 1;
            }

            if desc_id >= QSIZE as usize {
                RX_DIAG.skip_desc.fetch_add(1, Ordering::Relaxed);
            } else if total_len <= NET_HDR_LEN {
                RX_DIAG.skip_len.fetch_add(1, Ordering::Relaxed);
            } else {
                let virt = rx.buf_virt[desc_id] as *const u8;
                let frame = unsafe {
                    core::slice::from_raw_parts(virt.add(NET_HDR_LEN), total_len - NET_HDR_LEN)
                };
                cb(frame);
                delivered += 1;
                RX_DIAG.delivered.fetch_add(1, Ordering::Relaxed);
            }
            rx.last_used = rx.last_used.wrapping_add(1);
            *self.rx_done.lock() += 1;

            // Re-arm this RX descriptor by re-publishing it on the
            // avail ring (buffer is already populated and writable).
            unsafe {
                let avail_idx = (*rx.avail).idx.load(Ordering::Relaxed);
                (*rx.avail).ring[(avail_idx % QSIZE) as usize] = desc_id as u16;
                fence(Ordering::SeqCst);
                (*rx.avail)
                    .idx
                    .store(avail_idx.wrapping_add(1), Ordering::Release);
            }
            let mut notify: Port<u16> = Port::new(self.port_base + 0x10);
            unsafe {
                notify.write(0);
            }
        }
        delivered
    }

    /// Top-half for virtio-net PCI IRQ — drain RX used ring.
    pub fn irq_top_half(&self) {
        let isr = self.ack_isr();
        if (isr & 0x01) != 0 {
            // Use try_lock to avoid deadlocking if we interrupted a CPU
            // that is already actively polling the ring.
            if let Some(mut rx) = self.rx.try_lock() {
                // Drop the lock immediately so rx_poll can acquire it normally
                drop(rx);
                self.rx_poll(|_| {});
            }
        }
    }

    pub fn tx_count(&self) -> usize {
        *self.tx_done.lock()
    }
    pub fn rx_count(&self) -> usize {
        *self.rx_done.lock()
    }
    pub fn port_base(&self) -> u16 {
        self.port_base
    }
}

pub static VIRTIO_NET: Once<VirtioNet> = Once::new();

// ── Helpers ────────────────────────────────────────────────────────────

/// Allocate one physically-contiguous 4 KiB page; return (phys, virt).
unsafe fn alloc_page_pair() -> (u64, u64) {
    let mut alloc = GlobalFrameAllocator;
    let frame = alloc.allocate_frame().expect("[virtio-net] out of frames");
    let phys = frame.start_address().as_u64();
    let offset = *crate::memory::PHYS_MEM_OFFSET
        .get()
        .expect("PHYS_MEM_OFFSET");
    let virt = (offset + phys).as_u64();
    core::ptr::write_bytes(virt as *mut u8, 0, 4096);
    (phys, virt)
}

/// Allocate N contiguous 4 KiB pages via the buddy allocator. `order`
/// must satisfy 2^order ≥ N.
unsafe fn alloc_contig_pages(order: u8) -> Option<(u64, u64)> {
    let pa = crate::memory::allocate_contiguous_frames(order)?;
    let phys = pa.as_u64();
    let offset = *crate::memory::PHYS_MEM_OFFSET
        .get()
        .expect("PHYS_MEM_OFFSET");
    let virt = (offset + phys).as_u64();
    let bytes = 4096usize << order;
    core::ptr::write_bytes(virt as *mut u8, 0, bytes);
    Some((phys, virt))
}

/// Allocate a virtqueue (desc + avail + used) — needs 3 contiguous pages.
/// We request order 2 (= 4 pages = 16 KiB) so the buddy gives us a
/// power-of-two block; one page is wasted but that's cheap compared to
/// the alternative of stitching pages together.
unsafe fn alloc_virtqueue(port_base: u16, q_idx: u16) -> Option<VirtQueue> {
    let mut queue_select: Port<u16> = Port::new(port_base + 0x0E);
    let mut queue_size_port: PortReadOnly<u16> = PortReadOnly::new(port_base + 0x0C);
    let mut queue_addr: Port<u32> = Port::new(port_base + 0x08);
    queue_select.write(q_idx);
    let dev_qsize = queue_size_port.read();
    if dev_qsize < QSIZE {
        crate::serial_println!(
            "[virtio-net] queue {} size {} < {}",
            q_idx,
            dev_qsize,
            QSIZE
        );
        return None;
    }

    // 4 contiguous pages for desc + avail + used (+1 unused slack).
    let (p1, v1) = match alloc_contig_pages(2) {
        Some(x) => x,
        None => {
            crate::serial_println!("[virtio-net] queue {} contig alloc failed", q_idx);
            return None;
        }
    };

    let desc = v1 as *mut Desc;
    let avail = (v1 + 4096) as *mut AvailRing;
    let used = (v1 + 8192) as *mut UsedRing;

    // Tell device where the queue lives. Legacy format: 4-KiB-page-number.
    queue_addr.write((p1 / 4096) as u32);

    Some(VirtQueue {
        desc,
        avail,
        used,
        last_used: 0,
        buf_phys: [0; QSIZE as usize],
        buf_virt: [0; QSIZE as usize],
        queue_index: q_idx,
        queue_pfn: (p1 / 4096) as u32,
    })
}

// ── Init ───────────────────────────────────────────────────────────────

pub fn init(dev: &crate::pci::PciDevice) {
    if VIRTIO_NET.get().is_some() {
        return;
    }

    // BAR0 is the legacy I/O port range. Bottom bit set = I/O space.
    let bar0 = dev.bars[0];
    if bar0 & 1 == 0 {
        crate::serial_println!(
            "[virtio-net] BAR0 {:#x} is MMIO; legacy driver needs I/O. skipping",
            bar0
        );
        return;
    }
    let port_base = (bar0 & 0xFFFC) as u16;
    crate::serial_println!(
        "[virtio-net] {:02x}:{:02x}.{} BAR0=I/O@0x{:x} irq={}",
        dev.bus,
        dev.device,
        dev.function,
        port_base,
        dev.irq_line,
    );

    crate::pci::enable_bus_mastering(dev);

    // ── 1. Reset ───────────────────────────────────────────────────────
    let mut status: Port<u8> = Port::new(port_base + 0x12);
    unsafe {
        status.write(0);
    }

    // ── 2. ACKNOWLEDGE + DRIVER ───────────────────────────────────────
    unsafe {
        status.write(VIRTIO_STATUS_ACKNOWLEDGE);
    }
    unsafe {
        status.write(VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER);
    }

    // ── 3. Feature negotiation ────────────────────────────────────────
    let mut dev_feat: PortReadOnly<u32> = PortReadOnly::new(port_base + 0x00);
    let mut guest_feat: Port<u32> = Port::new(port_base + 0x04);
    let supported = unsafe { dev_feat.read() };
    let accepted = supported & VIRTIO_NET_F_MAC;
    unsafe {
        guest_feat.write(accepted);
    }
    crate::serial_println!(
        "[virtio-net] device features=0x{:08x} accepted=0x{:08x}",
        supported,
        accepted,
    );
    // Transitional QEMU virtio-net expects FEATURES_OK after feature write.
    // If the device clears the bit, fall back to pure legacy 0.9.
    let mut features_ok = false;
    if accepted != 0 {
        unsafe {
            status.write(
                VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_FEATURES_OK,
            );
        }
        let s = unsafe { status.read() };
        features_ok = (s & VIRTIO_STATUS_FEATURES_OK) != 0;
        crate::serial_println!(
            "[virtio-net] FEATURES_OK write -> status={:#04x} (ok={})",
            s,
            features_ok,
        );
    }

    // ── 4. Read MAC from config space (offset 0x14) ───────────────────
    let mac_ports: [PortReadOnly<u8>; 6] = [
        PortReadOnly::new(port_base + 0x14),
        PortReadOnly::new(port_base + 0x15),
        PortReadOnly::new(port_base + 0x16),
        PortReadOnly::new(port_base + 0x17),
        PortReadOnly::new(port_base + 0x18),
        PortReadOnly::new(port_base + 0x19),
    ];
    let mut mac = [0u8; 6];
    for i in 0..6 {
        // Port type is opaque — clone via Port::new on the raw port.
        let mut p: PortReadOnly<u8> = PortReadOnly::new(port_base + 0x14 + i as u16);
        mac[i] = unsafe { p.read() };
        let _ = &mac_ports;
    }
    crate::serial_println!(
        "[virtio-net] MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5],
    );

    // ── 5. Allocate RX (q0) and TX (q1) virtqueues ────────────────────
    let rx = unsafe { alloc_virtqueue(port_base, 0) };
    let tx = unsafe { alloc_virtqueue(port_base, 1) };
    let (mut rx, mut tx) = match (rx, tx) {
        (Some(r), Some(t)) => (r, t),
        _ => {
            crate::serial_println!("[virtio-net] virtqueue alloc failed");
            return;
        }
    };

    // ── 6. Pre-populate RX with QSIZE buffers ─────────────────────────
    unsafe {
        for i in 0..QSIZE {
            let (p, v) = alloc_page_pair();
            // Each RX descriptor: device-writable, full buffer length.
            let d = &mut *rx.desc.add(i as usize);
            d.addr = p;
            d.len = RX_BUF_LEN as u32;
            d.flags = VRING_DESC_F_WRITE;
            d.next = 0;
            rx.buf_phys[i as usize] = p;
            rx.buf_virt[i as usize] = v;
            (*rx.avail).ring[i as usize] = i;
        }
        (*rx.avail).idx.store(QSIZE, Ordering::Release);
    }

    // ── 7. Reserve TX descriptor 0 + a one-page TX buffer ─────────────
    unsafe {
        let (p, v) = alloc_page_pair();
        tx.buf_phys[0] = p;
        tx.buf_virt[0] = v;
        let d = &mut *tx.desc.add(0);
        d.addr = p;
        d.len = 0;
        d.flags = 0;
        d.next = 0;
    }

    // ── 8. DRIVER_OK — device is now live ─────────────────────────────
    let mut ok_status = VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_DRIVER_OK;
    if features_ok {
        ok_status |= VIRTIO_STATUS_FEATURES_OK;
    }
    unsafe {
        status.write(ok_status);
    }
    let live = unsafe { status.read() };
    crate::serial_println!("[virtio-net] DRIVER_OK -> status={:#04x}", live);

    // Kick RX so the device knows we have buffers waiting.
    let mut notify: Port<u16> = Port::new(port_base + 0x10);
    unsafe {
        notify.write(0);
    }

    let inst = VirtioNet {
        port_base,
        irq_line: dev.irq_line,
        mac,
        rx: Mutex::new(rx),
        tx: Mutex::new(tx),
        tx_done: spin::Mutex::new(0),
        rx_done: spin::Mutex::new(0),
    };
    VIRTIO_NET.call_once(|| inst);

    if dev.irq_line != 0 {
        crate::apic::route_irq(
            dev.irq_line as u32,
            crate::interrupts::InterruptIndex::VirtioNet.as_u8(),
        );
        crate::serial_println!(
            "[virtio-net] IRQ GSI {} -> vector {}",
            dev.irq_line,
            crate::interrupts::InterruptIndex::VirtioNet.as_u8(),
        );
    } else {
        crate::serial_println!("[virtio-net] WARN: irq_line=0 (poll-only RX)");
    }

    if let Some(net) = VIRTIO_NET.get() {
        net.dump_queue_state("post-init");
    }
    crate::serial_println!("[ OK ] virtio-net online (legacy I/O, 2 queues, RX pre-armed)");
}

// ── Boot smoketest ─────────────────────────────────────────────────────

/// Deep RX bring-up probe: dump rings, TX a frame, poll with ISR ack,
/// and report whether used.idx advanced vs frames delivered.
pub fn run_rx_bringup_probe() {
    let net = match VIRTIO_NET.get() {
        Some(n) => n,
        None => {
            crate::serial_println!("[virtio-net] rx-probe SKIP: no device");
            return;
        }
    };

    crate::serial_println!("[virtio-net] === RX bring-up probe start ===");
    net.dump_queue_state("probe-pre-tx");

    let src = net.mac();
    let mut frame = [0u8; 60];
    frame[0..6].copy_from_slice(&[0xff; 6]);
    frame[6..12].copy_from_slice(&src);
    frame[12] = 0x08;
    frame[13] = 0x06; // ARP — QEMU user netdev usually answers ARP/broadcast

    match net.tx_frame(&frame) {
        Ok(()) => crate::serial_println!("[virtio-net] probe: ARP broadcast TX ok"),
        Err(e) => crate::serial_println!("[virtio-net] probe: TX failed: {}", e),
    }

    net.dump_queue_state("probe-post-tx");

    let mut poll_delivered = 0usize;
    for pass in 0..32 {
        let got = net.rx_poll(|eth| {
            if eth.len() >= 14 {
                crate::serial_println!(
                    "[virtio-net] probe rx {}B ethertype={:04x}",
                    eth.len(),
                    ((eth[12] as u16) << 8) | eth[13] as u16,
                );
            }
        });
        poll_delivered += got;
        if pass == 0 || pass == 7 || pass == 31 || got > 0 {
            crate::serial_println!(
                "[virtio-net] probe poll pass={} delivered_this={}",
                pass,
                got
            );
            net.dump_queue_state("probe-poll");
        }
        for _ in 0..100_000 {
            core::hint::spin_loop();
        }
    }

    let used_adv = RX_DIAG.used_advanced.load(Ordering::Relaxed);
    let delivered = RX_DIAG.delivered.load(Ordering::Relaxed);
    let skip_len = RX_DIAG.skip_len.load(Ordering::Relaxed);
    let skip_desc = RX_DIAG.skip_desc.load(Ordering::Relaxed);

    // QEMU user netdev responds to DHCP, not necessarily to bare ARP probes.
    crate::serial_println!(
        "[virtio-net] probe: kicking DHCPDISCOVER to generate inbound traffic..."
    );
    let _ = crate::dhcp::kick_discovery(0);
    for pass in 0..64 {
        let got = net.rx_poll(|eth| {
            if eth.len() >= 14 {
                crate::serial_println!(
                    "[virtio-net] probe dhcp-rx {}B ethertype={:04x}",
                    eth.len(),
                    ((eth[12] as u16) << 8) | eth[13] as u16,
                );
            }
        });
        poll_delivered += got;
        if got > 0 {
            net.dump_queue_state("probe-dhcp-rx");
        }
        crate::net::poll();
        for _ in 0..20_000 {
            core::hint::spin_loop();
        }
    }

    let used_adv = RX_DIAG.used_advanced.load(Ordering::Relaxed);
    let delivered = RX_DIAG.delivered.load(Ordering::Relaxed);
    let skip_len = RX_DIAG.skip_len.load(Ordering::Relaxed);
    let skip_desc = RX_DIAG.skip_desc.load(Ordering::Relaxed);
    let dhcp_state = crate::dhcp::current_state();

    if used_adv == 0 {
        crate::serial_println!(
            "[virtio-net] probe VERDICT: used.idx NEVER advanced — no virtio RX completions (ring/notify/DMA bug)",
        );
    } else if delivered == 0 {
        crate::serial_println!(
            "[virtio-net] probe VERDICT: used.idx advanced {} times but 0 delivered (parse/drop bug: skip_len={} skip_desc={})",
            used_adv, skip_len, skip_desc,
        );
    } else {
        crate::serial_println!(
            "[virtio-net] probe VERDICT: OK — {} used completions, {} delivered (poll_total={} dhcp_state={:?})",
            used_adv, delivered, poll_delivered, dhcp_state,
        );
    }
    crate::serial_println!("[virtio-net] === RX bring-up probe end ===");
}

pub fn run_boot_smoketest() {
    let net = match VIRTIO_NET.get() {
        Some(n) => n,
        None => {
            crate::serial_println!("[virtio-net] smoketest SKIP: no device");
            return;
        }
    };

    // Build a broadcast frame with an experimental EtherType (0x88b5).
    let src = net.mac();
    let mut frame = [0u8; 60];
    frame[0..6].copy_from_slice(&[0xff; 6]);
    frame[6..12].copy_from_slice(&src);
    frame[12] = 0x88;
    frame[13] = 0xb5;
    let tag = b"AthenaOS-virtio-net-hello";
    frame[14..14 + tag.len()].copy_from_slice(tag);

    let t0 = unsafe { core::arch::x86_64::_rdtsc() };
    let res = net.tx_frame(&frame);
    let t1 = unsafe { core::arch::x86_64::_rdtsc() };

    match res {
        Ok(()) => crate::serial_println!(
            "[virtio-net] smoketest: TX broadcast 60B in {} cycles, tx_done={}",
            t1.saturating_sub(t0),
            net.tx_count(),
        ),
        Err(e) => crate::serial_println!("[virtio-net] smoketest FAIL: {}", e),
    }

    // QEMU's user-mode network will respond to broadcast with RARP-ish
    // chatter on most setups. We poll a few times so we can confirm RX
    // path works end-to-end without depending on a specific reply.
    let mut got = 0usize;
    for _ in 0..1000 {
        got += net.rx_poll(|frame| {
            crate::serial_println!(
                "[virtio-net] rx {}B from {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                frame.len(),
                frame.get(6).copied().unwrap_or(0),
                frame.get(7).copied().unwrap_or(0),
                frame.get(8).copied().unwrap_or(0),
                frame.get(9).copied().unwrap_or(0),
                frame.get(10).copied().unwrap_or(0),
                frame.get(11).copied().unwrap_or(0),
            );
        });
        if got > 0 {
            break;
        }
        for _ in 0..1000 {
            core::hint::spin_loop();
        }
    }
    crate::serial_println!(
        "[virtio-net] smoketest: rx_done={} (0 is normal on isolated QEMU)",
        net.rx_count(),
    );
}
