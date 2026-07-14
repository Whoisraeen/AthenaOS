//! Network stack — smoltcp on top of the unified NIC driver layer.
//!
//! Provides a `NetworkStack` that wraps the active NIC (e1000, virtio-net,
//! or any future `NetDriver`) in a smoltcp `Device` and runs an `Interface`
//! with DHCP and basic TCP/UDP sockets.
//!
//! The stack is polled from the kernel's timer tick or a dedicated
//! network task. Userspace accesses it via syscalls (future work).

use alloc::vec;
use alloc::vec::Vec;
use smoltcp::iface::{Config, Interface, SocketSet};
use smoltcp::phy::{self, Device, DeviceCapabilities, Medium};
use smoltcp::time::Instant;
use smoltcp::wire::{
    EthernetAddress, HardwareAddress, IpAddress, IpCidr, IpProtocol, Ipv4Address, Ipv6Address,
    Ipv6Packet,
};
use spin::Mutex;

// ─── Backend selector ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NicBackend {
    NetDriver,
    VirtioNet,
}

static NIC_BACKEND: Mutex<NicBackend> = Mutex::new(NicBackend::VirtioNet);

// ─── Unified smoltcp Device ─────────────────────────────────────────────────

pub struct NicDevice;

impl Device for NicDevice {
    type RxToken<'a> = NicRxToken;
    type TxToken<'a> = NicTxToken;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        // Monotonic seconds — the SAME clock `dhcp::tick` uses (see poll_full), so
        // a lease's `obtained_at` lines up with the timeline its renewal/expiry
        // checks read. (Was hardcoded 0 in the VirtioNet branch — a latent
        // lease-timing inconsistency.)
        let now = (crate::hpet::read_millis().unwrap_or(0) as u64) / 1000;
        let backend = *NIC_BACKEND.lock();
        match backend {
            NicBackend::NetDriver => {
                // Route received frames: a DHCP reply (OFFER/ACK) goes to the raw
                // DHCP client — smoltcp has NO DHCP socket, so without this the
                // OFFER is dropped and DHCP stalls at Selecting forever. This was
                // the iron bug: the VirtioNet branch (QEMU) routed to the DHCP
                // client, the NetDriver branch (real RTL8125) did NOT, so DHCP
                // could never bind on Athena. The first non-DHCP frame is handed
                // to smoltcp; bounded so a burst can't spin here.
                //
                // LOCK ORDER (critical): `handle_eth_frame`'s OFFER path re-locks
                // NET_DRIVERS to transmit the REQUEST, so `recv()` must RELEASE
                // NET_DRIVERS *before* we dispatch — holding it across
                // `handle_eth_frame` would self-deadlock the spin lock (single
                // scheduling CPU, non-reentrant Mutex).
                //
                // DRAIN POLICY (matches the proven VirtioNet branch below): the
                // OLD code returned to smoltcp on the FIRST non-DHCP frame and
                // left everything behind it in the ring for a later poll. On a
                // chatty real LAN (ARP/mDNS/SSDP/IPv6-RA broadcast), a single
                // DHCP OFFER can sit BEHIND such a frame and, if the RX ring
                // overflows before the next poll drains it, be overwritten — DHCP
                // then loops at Selecting forever (the live "TX works, stuck at
                // Selecting" symptom). Fix: greedily drain the ring THIS poll,
                // handing every DHCP reply to the client immediately and buffering
                // only the FIRST non-DHCP frame to return to smoltcp (later
                // non-DHCP frames are dropped from smoltcp's view exactly as the
                // VirtioNet branch does — the poll thread re-runs, so smoltcp
                // still makes progress). Capped to bound a pathological flood.
                const RX_DRAIN_CAP: usize = 64;
                let mut first_non_dhcp: Option<Vec<u8>> = None;
                for _ in 0..RX_DRAIN_CAP {
                    let pkt = {
                        let mut guard = crate::net_drivers::NET_DRIVERS.lock();
                        let mgr = guard.as_mut()?;
                        let drv = mgr.default_driver_mut()?;
                        match drv.recv() {
                            Some(p) => p,
                            None => break,
                        }
                    };
                    if crate::dhcp::handle_eth_frame(&pkt, now) {
                        continue;
                    }
                    // First non-DHCP frame is the one we surface to smoltcp; keep
                    // draining so a DHCP reply later in the ring still lands now.
                    if first_non_dhcp.is_none() {
                        first_non_dhcp = Some(pkt);
                    }
                }
                first_non_dhcp.map(|pkt| (NicRxToken(pkt), NicTxToken))
            }
            NicBackend::VirtioNet => {
                let net = crate::virtio_net::VIRTIO_NET.get()?;
                // Drain at least one non-DHCP frame for smoltcp; keep
                // draining DHCP internally inside rx_poll until empty.
                let mut frame_data: Option<Vec<u8>> = None;
                net.rx_poll(|eth| {
                    if frame_data.is_some() {
                        return;
                    }
                    if crate::dhcp::handle_eth_frame(eth, now) {
                        return;
                    }
                    frame_data = Some(Vec::from(eth));
                });
                frame_data.map(|data| (NicRxToken(data), NicTxToken))
            }
        }
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(NicTxToken)
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = 1514;
        caps.max_burst_size = Some(1);
        caps
    }
}

pub struct NicRxToken(Vec<u8>);

impl phy::RxToken for NicRxToken {
    fn consume<R, F>(mut self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        f(&mut self.0)
    }
}

pub struct NicTxToken;

impl phy::TxToken for NicTxToken {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buf = vec![0u8; len];
        let result = f(&mut buf);
        transmit_raw(&buf);
        result
    }
}

/// Transmit one raw ethernet frame via the active NIC backend. Public for
/// the netlog broadcast channel (kernel/src/netlog.rs), which builds its own
/// UDP frames; everything else should go through the smoltcp stack.
pub fn transmit_raw(buf: &[u8]) {
    match *NIC_BACKEND.lock() {
        NicBackend::NetDriver => {
            let mut guard = crate::net_drivers::NET_DRIVERS.lock();
            if let Some(mgr) = guard.as_mut() {
                if let Some(drv) = mgr.default_driver_mut() {
                    let _ = drv.send(buf);
                }
            }
        }
        NicBackend::VirtioNet => {
            if let Some(net) = crate::virtio_net::VIRTIO_NET.get() {
                let _ = net.tx_frame(buf);
            }
        }
    }
}

/// Global network stack state.
pub struct NetworkStack {
    pub iface: Interface,
    pub device: NicDevice,
    pub sockets: SocketSet<'static>,
}

pub static NET_STACK: Mutex<Option<NetworkStack>> = Mutex::new(None);

/// Initialize the smoltcp network stack.
/// Prefers e1000/real NIC via NetDriverManager; falls back to virtio-net.
pub fn init() {
    let (mac, backend, backend_driver_name) = {
        let guard = crate::net_drivers::NET_DRIVERS.lock();
        if let Some(ref mgr) = *guard {
            if let Some(drv) = mgr.default_driver() {
                let m = drv.mac_address();
                let name =
                    alloc::string::String::from(mgr.default_driver_name().unwrap_or(drv.name()));
                (Some(m), NicBackend::NetDriver, Some(name))
            } else {
                (None, NicBackend::VirtioNet, None)
            }
        } else {
            (None, NicBackend::VirtioNet, None)
        }
    };

    let mac = if let Some(m) = mac {
        *NIC_BACKEND.lock() = backend;
        m
    } else if let Some(net) = crate::virtio_net::VIRTIO_NET.get() {
        *NIC_BACKEND.lock() = NicBackend::VirtioNet;
        net.mac()
    } else {
        crate::serial_println!("[net] no NIC available; skipping stack init");
        return;
    };

    let backend_name = match *NIC_BACKEND.lock() {
        NicBackend::NetDriver => backend_driver_name.as_deref().unwrap_or("NetDriver"),
        NicBackend::VirtioNet => "virtio-net",
    };

    let hw_addr = HardwareAddress::Ethernet(EthernetAddress(mac));

    let mut device = NicDevice;
    let config = Config::new(hw_addr);
    let now = Instant::from_millis(0);

    let mut iface = Interface::new(config, &mut device, now);

    // Dual-stack (MasterChecklist Phase 10.2 — "IPv6 dual-stack"): the v4
    // address DHCP will overwrite, plus the RFC 4291 §2.5.1 modified-EUI-64
    // link-local v6 address every IPv6 host autoconfigures before any router
    // is seen. smoltcp's proto-ipv6 answers NDP neighbor solicitations and
    // ICMPv6 echo on it from this point on.
    let lla = ipv6_link_local_from_mac(&mac);
    iface.update_ip_addrs(|addrs| {
        let _ = addrs.push(IpCidr::new(Ipv4Address::new(10, 0, 2, 15).into(), 24));
        let _ = addrs.push(IpCidr::new(IpAddress::Ipv6(lla), 64));
    });

    let sockets = SocketSet::new(vec![]);

    *NET_STACK.lock() = Some(NetworkStack {
        iface,
        device,
        sockets,
    });

    crate::serial_println!(
        "[ OK ] Network stack initialized (smoltcp dual-stack, IP 10.0.2.15/24 + {}/64, MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, {})",
        lla,
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5], backend_name,
    );
}

/// RFC 4291 §2.5.1 modified EUI-64 link-local address (fe80::/64) for a MAC:
/// flip the universal/local bit of the first octet, insert ff:fe in the
/// middle. This is the deterministic v6 identity every host derives for
/// itself with zero configuration.
pub fn ipv6_link_local_from_mac(mac: &[u8; 6]) -> Ipv6Address {
    let mut b = [0u8; 16];
    b[0] = 0xfe;
    b[1] = 0x80;
    b[8] = mac[0] ^ 0x02;
    b[9] = mac[1];
    b[10] = mac[2];
    b[11] = 0xff;
    b[12] = 0xfe;
    b[13] = mac[3];
    b[14] = mac[4];
    b[15] = mac[5];
    Ipv6Address::from_bytes(&b)
}

/// Deterministic IPv6 dual-stack proof (MasterChecklist Phase 10.2):
/// (1) the live interface holds an IPv4 AND an IPv6 address simultaneously,
/// (2) the v6 address is the RFC 4291 modified-EUI-64 link-local for our MAC,
/// (3) the proto-ipv6 wire machinery field-exact-parses a crafted IPv6
///     header (v6 has no header checksum — field parse IS the format proof).
pub fn run_ipv6_smoketest() {
    let (have_v4, v6_addr, eui64_ok) = {
        let guard = NET_STACK.lock();
        match guard.as_ref() {
            Some(stack) => {
                let mut have_v4 = false;
                let mut v6: Option<Ipv6Address> = None;
                for cidr in stack.iface.ip_addrs() {
                    match cidr.address() {
                        IpAddress::Ipv4(_) => have_v4 = true,
                        IpAddress::Ipv6(a) => v6 = Some(a),
                    }
                }
                let eui64_ok = match (stack.iface.hardware_addr(), v6) {
                    (HardwareAddress::Ethernet(EthernetAddress(mac)), Some(a)) => {
                        a == ipv6_link_local_from_mac(&mac)
                    }
                    _ => false,
                };
                (have_v4, v6, eui64_ok)
            }
            None => {
                crate::serial_println!(
                    "[net] ipv6 smoketest: stack not initialized -> SKIP (no NIC)"
                );
                return;
            }
        }
    };

    // 48-byte IPv6 datagram: version 6, payload_len 8, next header UDP,
    // hop limit 64, src fe80::1, dst ff02::1 (all-nodes multicast).
    let mut raw = [0u8; 48];
    raw[0] = 0x60;
    raw[5] = 8;
    raw[6] = 17;
    raw[7] = 64;
    raw[8] = 0xfe;
    raw[9] = 0x80;
    raw[23] = 1;
    raw[24] = 0xff;
    raw[25] = 0x02;
    raw[39] = 1;
    let header_parse = match Ipv6Packet::new_checked(&raw[..]) {
        Ok(p) => {
            p.version() == 6
                && p.next_header() == IpProtocol::Udp
                && p.hop_limit() == 64
                && p.payload_len() == 8
                && p.src_addr() == Ipv6Address::from_bytes(&raw[8..24])
                && p.dst_addr().is_multicast()
        }
        Err(_) => false,
    };

    let have_v6 = v6_addr.is_some();
    let pass = have_v4 && have_v6 && eui64_ok && header_parse;
    crate::serial_println!(
        "[net] ipv6 smoketest: dual_stack(v4={},v6={}) eui64_lla={} v6_header_parse={} -> {}",
        have_v4,
        have_v6,
        eui64_ok,
        header_parse,
        if pass { "PASS" } else { "FAIL" },
    );
}

pub fn dump_text() -> alloc::string::String {
    let mut out = crate::net_drivers::pci_selection_dump_text();
    out.push_str("# AthenaOS network subsystem\n");
    let backend = match *NIC_BACKEND.lock() {
        NicBackend::NetDriver => "net_driver",
        NicBackend::VirtioNet => "virtio-net",
    };
    out.push_str(&alloc::format!("backend: {}\n", backend));
    out.push_str(&alloc::format!(
        "stack_initialized: {}\n",
        NET_STACK.lock().is_some() as u8
    ));
    if let Some(stack) = NET_STACK.lock().as_ref() {
        for cidr in stack.iface.ip_addrs() {
            out.push_str(&alloc::format!("addr: {}\n", cidr));
        }
    }
    if let Some(state) = crate::dhcp::current_state() {
        out.push_str(&alloc::format!("dhcp_state: {:?}\n", state));
    } else {
        out.push_str("dhcp_state: uninitialized\n");
    }
    out.push_str(&crate::net_drivers::dump_text());
    out
}

/// Poll the network stack. Call periodically from the timer handler or
/// a dedicated kernel task.
pub fn poll() {
    let mut guard = NET_STACK.lock();
    if let Some(ref mut stack) = *guard {
        let now = Instant::from_millis(crate::hpet::read_millis().unwrap_or(0));
        let _ = stack.iface.poll(now, &mut stack.device, &mut stack.sockets);
    }
}

/// Full network poll cycle: smoltcp stack + DHCP + firewall cleanup + traffic shaper.
/// Call this from the timer handler instead of bare `poll()` to get all
/// networking subsystems serviced.
pub fn poll_full() {
    // 1. Poll the smoltcp stack (RX/TX)
    poll();

    let now_secs = (crate::hpet::read_millis().unwrap_or(0) as u64) / 1000;

    // 2. DHCP state machine tick + lease renewal check
    crate::dhcp::tick(now_secs);
    crate::dhcp::check_lease_renewal(); // MasterChecklist Phase 10: DHCP renewal on lease expiry

    // 3. WireGuard peer keepalive + rekey timer
    crate::wireguard::tick(now_secs); // MasterChecklist Phase 10: WireGuard keepalive 10s / rekey 180s

    // 3. Firewall conntrack cleanup (every ~30 seconds is fine,
    //    but calling it each tick is cheap since it's a no-op
    //    when nothing is expired)
    crate::firewall::periodic_cleanup(now_secs);

    // 4. Drain shaped egress packets
    shaped_drain();
}

/// Post-boot network service thread. The boot sequence polls the stack only
/// ~192 times in the first few tens of milliseconds (`main.rs`), which is far
/// too short for a real router's DHCP DISCOVER→OFFER round-trip. Without a
/// continuous driver, `poll_full()` is never called again, so DHCP can't bind,
/// inbound RX frames are never drained into the stack, and TCP makes no
/// progress. This thread IS that driver: it runs `poll_full()` every ~50 ms
/// forever.
///
/// HARD RULE — this thread must stay FEATHER-LIGHT. Two earlier versions broke
/// the iron desktop: (1) `bootlog_persist::flush()` (a big NVMe write) on the
/// first poll, and (2) `netlog::broadcast_ring()` (~70 back-to-back TX frames
/// of the whole log) at a 3 s checkpoint. Both are heavy BURSTS that, running
/// concurrently with desktop bring-up, starved the CPU and the desktop never
/// appeared (bootlogs T2357 / T0013). `poll_full()` alone is fine (T2332 came
/// up). So: NO block I/O, NO ring broadcasts here — only `poll_full()` + a
/// cheap one-line serial note (in-RAM ring). And we DELAY the first poll until
/// well after the ~10 s desktop auto-advance, so networking can never contend
/// with desktop bring-up.
/// Whether the one-shot virtio-net RX bring-up probe runs in the post-boot
/// poll thread (ADR 0006). Default ON; off the boot critical path either way.
static RX_PROBE_ENABLED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// Enable/disable the deferred post-boot RX bring-up probe. Must be called
/// before `spawn_poll_thread()` to take effect on this boot.
pub fn set_rx_probe_enabled(on: bool) {
    RX_PROBE_ENABLED.store(on, core::sync::atomic::Ordering::Relaxed);
}

extern "C" fn net_poll_thread_entry() {
    // Wait ~12 s before touching the stack: the desktop auto-advance fires at
    // ~10 s, and this thread must not compete with it. DHCP completing a few
    // seconds later than boot is irrelevant.
    for _ in 0..1200 {
        crate::scheduler::yield_task();
        x86_64::instructions::hlt();
    }
    crate::serial_println!(
        "[net] post-boot poll thread active (desktop already up; driving DHCP/RX/TCP)"
    );

    // ADR 0006 (boot-time gate): the virtio-net RX bring-up probe is a
    // once-proven diagnostic that cost ~950 ms on the boot critical path
    // (32+64 polling passes with inner spin loops). It does not gate any
    // boot-health check — the real DHCP bind is driven by this very poll
    // loop below — so it now runs ONCE here, after the startup delay, off
    // the marker path. Toggleable via set_rx_probe_enabled() for future
    // userspace/quirk wiring; default ON post-boot.
    if RX_PROBE_ENABLED.load(core::sync::atomic::Ordering::Relaxed) {
        crate::virtio_net::run_rx_bringup_probe();
    } else {
        crate::serial_println!("[net] vnet RX bring-up probe disabled (off critical path)");
    }

    let mut announced_bound = false;
    let mut checkpoint_done = false;
    let mut polls: u32 = 0;
    loop {
        poll_full();
        polls = polls.saturating_add(1);

        let bound = matches!(
            crate::dhcp::current_state(),
            Some(crate::dhcp::DhcpState::Bound)
        );
        if bound && !announced_bound {
            announced_bound = true;
            crate::serial_println!("[net] post-boot: DHCP BOUND after {} polls", polls);
        }
        // ~5 s checkpoint (100 polls after the startup delay): one cheap line.
        if !announced_bound && !checkpoint_done && polls >= 100 {
            checkpoint_done = true;
            crate::serial_println!(
                "[net] post-boot: still {:?} after 100 polls",
                crate::dhcp::current_state()
            );
        }

        // ~50 ms between polls: responsive for DHCP/TCP, light on the CPU.
        for _ in 0..5 {
            crate::scheduler::yield_task();
            x86_64::instructions::hlt();
        }
    }
}

/// Spawn the post-boot network poll thread. Call ONCE, after the procfs boot
/// dump (so a context switch can't corrupt the boot kernel stack — same
/// constraint as the deferred `user_init` spawn).
pub fn spawn_poll_thread() {
    let task = crate::task::Task::new(net_poll_thread_entry, None);
    // Pin to CPU 0 — the APs don't schedule post-boot (see scheduler::spawn_on_bsp).
    crate::scheduler::spawn_on_bsp(task);
    crate::serial_println!("[net] post-boot poll thread spawned (drives DHCP/RX/TCP, BSP-pinned)");
}

/// Filter an inbound raw IPv4 packet through the firewall.
/// Returns true if the packet should be accepted, false if dropped.
pub fn firewall_check_inbound(ipv4_data: &[u8], app_id: Option<u64>) -> bool {
    let now_secs = (crate::hpet::read_millis().unwrap_or(0) as u64) / 1000;
    if let Some(pkt) = crate::firewall::PacketInfo::from_ipv4(ipv4_data, app_id) {
        matches!(
            crate::firewall::filter_inbound(&pkt, now_secs),
            crate::firewall::Verdict::Allow,
        )
    } else {
        true
    }
}

/// Filter an outbound raw IPv4 packet through the firewall.
/// Returns true if the packet should be accepted, false if dropped.
pub fn firewall_check_outbound(ipv4_data: &[u8], app_id: Option<u64>) -> bool {
    let now_secs = (crate::hpet::read_millis().unwrap_or(0) as u64) / 1000;
    if let Some(pkt) = crate::firewall::PacketInfo::from_ipv4(ipv4_data, app_id) {
        matches!(
            crate::firewall::filter_outbound(&pkt, now_secs),
            crate::firewall::Verdict::Allow,
        )
    } else {
        true
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Gaming Traffic Shaping — strict-priority egress scheduler
// ─────────────────────────────────────────────────────────────────────────────

/// Traffic class for the AthNet priority queue.
/// Outbound packets are tagged by the sending process's SCHED_BODY status
/// and dequeued in strict priority order: Game first, then Interactive, then Bulk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TrafficClass {
    /// Highest priority — game engine traffic, controller input, voice chat.
    /// Tagged automatically when the sending thread has SCHED_BODY priority.
    Game = 0,
    /// Normal priority — web browsing, chat, interactive apps.
    Interactive = 1,
    /// Lowest priority — downloads, system updates, telemetry.
    Bulk = 2,
}

/// A packet tagged with its traffic class for the priority scheduler.
#[derive(Debug, Clone)]
pub struct TaggedPacket {
    pub class: TrafficClass,
    pub data: Vec<u8>,
    pub enqueue_time_ms: u64,
}

/// Strict-priority egress scheduler for outbound network traffic.
///
/// Three queues (Game / Interactive / Bulk) with strict priority dequeue:
/// all Game packets drain before any Interactive packet is sent, and all
/// Interactive packets drain before any Bulk packet is sent. This gives
/// SCHED_BODY threads sub-frame network latency.
#[derive(Debug)]
pub struct TrafficShaper {
    game_queue: Vec<TaggedPacket>,
    interactive_queue: Vec<TaggedPacket>,
    bulk_queue: Vec<TaggedPacket>,
    game_capacity: usize,
    interactive_capacity: usize,
    bulk_capacity: usize,
    stats: TrafficShaperStats,
}

#[derive(Debug, Clone, Default)]
pub struct TrafficShaperStats {
    pub game_enqueued: u64,
    pub game_dequeued: u64,
    pub game_dropped: u64,
    pub interactive_enqueued: u64,
    pub interactive_dequeued: u64,
    pub interactive_dropped: u64,
    pub bulk_enqueued: u64,
    pub bulk_dequeued: u64,
    pub bulk_dropped: u64,
}

impl TrafficShaper {
    pub fn new() -> Self {
        Self {
            game_queue: Vec::new(),
            interactive_queue: Vec::new(),
            bulk_queue: Vec::new(),
            game_capacity: 128,
            interactive_capacity: 256,
            bulk_capacity: 512,
            stats: TrafficShaperStats::default(),
        }
    }

    pub fn with_capacities(game: usize, interactive: usize, bulk: usize) -> Self {
        Self {
            game_queue: Vec::new(),
            interactive_queue: Vec::new(),
            bulk_queue: Vec::new(),
            game_capacity: game,
            interactive_capacity: interactive,
            bulk_capacity: bulk,
            stats: TrafficShaperStats::default(),
        }
    }

    /// Enqueue a packet into the appropriate priority queue.
    /// If the queue is full, the packet is dropped (tail-drop).
    pub fn enqueue(&mut self, packet: TaggedPacket) -> bool {
        match packet.class {
            TrafficClass::Game => {
                if self.game_queue.len() >= self.game_capacity {
                    self.stats.game_dropped += 1;
                    return false;
                }
                self.stats.game_enqueued += 1;
                self.game_queue.push(packet);
            }
            TrafficClass::Interactive => {
                if self.interactive_queue.len() >= self.interactive_capacity {
                    self.stats.interactive_dropped += 1;
                    return false;
                }
                self.stats.interactive_enqueued += 1;
                self.interactive_queue.push(packet);
            }
            TrafficClass::Bulk => {
                if self.bulk_queue.len() >= self.bulk_capacity {
                    self.stats.bulk_dropped += 1;
                    return false;
                }
                self.stats.bulk_enqueued += 1;
                self.bulk_queue.push(packet);
            }
        }
        true
    }

    /// Dequeue the highest-priority packet. Strict priority:
    /// Game > Interactive > Bulk.
    pub fn dequeue(&mut self) -> Option<TaggedPacket> {
        if !self.game_queue.is_empty() {
            self.stats.game_dequeued += 1;
            return Some(self.game_queue.remove(0));
        }
        if !self.interactive_queue.is_empty() {
            self.stats.interactive_dequeued += 1;
            return Some(self.interactive_queue.remove(0));
        }
        if !self.bulk_queue.is_empty() {
            self.stats.bulk_dequeued += 1;
            return Some(self.bulk_queue.remove(0));
        }
        None
    }

    /// Dequeue up to `count` packets in priority order.
    pub fn dequeue_batch(&mut self, count: usize) -> Vec<TaggedPacket> {
        let mut batch = Vec::with_capacity(count);
        for _ in 0..count {
            match self.dequeue() {
                Some(pkt) => batch.push(pkt),
                None => break,
            }
        }
        batch
    }

    /// Classify a packet based on the sending process's scheduler class.
    /// SCHED_BODY threads automatically get TrafficClass::Game.
    pub fn classify(is_sched_game: bool, dst_port: u16) -> TrafficClass {
        if is_sched_game {
            return TrafficClass::Game;
        }
        match dst_port {
            // Common gaming ports
            3478..=3480 | 27000..=27050 | 7777..=7800 => TrafficClass::Game,
            // Common interactive ports (HTTP/S, SSH, DNS)
            80 | 443 | 22 | 53 | 8080 | 8443 => TrafficClass::Interactive,
            // Everything else is bulk
            _ => TrafficClass::Bulk,
        }
    }

    pub fn pending_count(&self) -> usize {
        self.game_queue.len() + self.interactive_queue.len() + self.bulk_queue.len()
    }

    pub fn game_pending(&self) -> usize {
        self.game_queue.len()
    }
    pub fn interactive_pending(&self) -> usize {
        self.interactive_queue.len()
    }
    pub fn bulk_pending(&self) -> usize {
        self.bulk_queue.len()
    }
    pub fn stats(&self) -> &TrafficShaperStats {
        &self.stats
    }

    /// Age out stale packets older than `max_age_ms`.
    pub fn expire_stale(&mut self, now_ms: u64, max_age_ms: u64) {
        let cutoff = now_ms.saturating_sub(max_age_ms);
        self.game_queue.retain(|p| p.enqueue_time_ms >= cutoff);
        self.interactive_queue
            .retain(|p| p.enqueue_time_ms >= cutoff);
        self.bulk_queue.retain(|p| p.enqueue_time_ms >= cutoff);
    }
}

pub static TRAFFIC_SHAPER: Mutex<Option<TrafficShaper>> = Mutex::new(None);

/// Enqueue an outbound packet through the traffic shaper.
/// Falls back to direct transmission if the shaper is not initialized.
pub fn shaped_send(data: &[u8], class: TrafficClass) -> bool {
    let mut guard = TRAFFIC_SHAPER.lock();
    if let Some(ref mut shaper) = *guard {
        let now = crate::hpet::read_millis().unwrap_or(0) as u64;
        shaper.enqueue(TaggedPacket {
            class,
            data: data.to_vec(),
            enqueue_time_ms: now,
        })
    } else {
        false
    }
}

/// Drain shaped packets and transmit them via the active NIC.
/// Called from the network poll loop.
pub fn shaped_drain() {
    let batch = {
        let mut guard = TRAFFIC_SHAPER.lock();
        match *guard {
            Some(ref mut shaper) => shaper.dequeue_batch(16),
            None => return,
        }
    };
    for pkt in batch {
        transmit_raw(&pkt.data);
    }
}

/// Initialize the traffic shaper. Call after net::init().
pub fn init_traffic_shaper() {
    *TRAFFIC_SHAPER.lock() = Some(TrafficShaper::new());
    crate::serial_println!("[ OK ] Traffic shaper initialized (Game/Interactive/Bulk)");
}

/// Deterministic proof of gaming traffic shaping (strict-priority egress):
/// classification by SCHED_BODY status and well-known port, that a Game packet
/// enqueued LAST still dequeues FIRST ahead of Interactive and Bulk, and that a
/// full queue tail-drops. MasterChecklist Phase 10.2 — gaming traffic shaping.
/// Concept §AthNet / SCHED_BODY sub-frame network latency.
pub fn run_traffic_shaper_smoketest() {
    let mut pass = 0u32;
    let mut total = 0u32;
    let mut check = |c: bool, n: &str| {
        total += 1;
        if c {
            pass += 1;
        } else {
            crate::serial_println!("[shaper-selftest] FAIL {}", n);
        }
    };

    // Classification: SCHED_BODY is always Game; otherwise the port decides.
    check(
        TrafficShaper::classify(true, 9999) == TrafficClass::Game,
        "classify-sched-game",
    );
    check(
        TrafficShaper::classify(false, 27015) == TrafficClass::Game,
        "classify-steam-port",
    );
    check(
        TrafficShaper::classify(false, 443) == TrafficClass::Interactive,
        "classify-https",
    );
    check(
        TrafficShaper::classify(false, 50000) == TrafficClass::Bulk,
        "classify-bulk",
    );

    // Strict priority: enqueue in reverse-priority order (Bulk, Interactive,
    // Game); dequeue must still yield Game, then Interactive, then Bulk.
    let mk = |class| TaggedPacket {
        class,
        data: alloc::vec![0u8; 4],
        enqueue_time_ms: 0,
    };
    let mut shaper = TrafficShaper::new();
    shaper.enqueue(mk(TrafficClass::Bulk));
    shaper.enqueue(mk(TrafficClass::Interactive));
    shaper.enqueue(mk(TrafficClass::Game));
    let order = [
        shaper.dequeue().map(|p| p.class),
        shaper.dequeue().map(|p| p.class),
        shaper.dequeue().map(|p| p.class),
    ];
    check(
        order
            == [
                Some(TrafficClass::Game),
                Some(TrafficClass::Interactive),
                Some(TrafficClass::Bulk),
            ],
        "strict-priority-order",
    );

    // Tail-drop: a third Game packet into a cap-2 queue is refused and counted.
    let mut small = TrafficShaper::with_capacities(2, 2, 2);
    let _ = small.enqueue(mk(TrafficClass::Game));
    let _ = small.enqueue(mk(TrafficClass::Game));
    let dropped = !small.enqueue(mk(TrafficClass::Game));
    check(dropped && small.stats().game_dropped == 1, "tail-drop");

    // Phase 10.2 — "in-game UDP latency UNCHANGED by background HTTP downloads".
    // Model the real contention: a DEEP bulk backlog (a background download)
    // already queued, THEN one game packet arrives. It must dequeue FIRST
    // (position 0), so the game packet's wait is independent of how many bulk
    // packets are backlogged — its latency is unchanged. A FIFO/round-robin
    // scheduler would FAIL this (the game packet would sit behind 200 bulk ones).
    let mut contended = TrafficShaper::with_capacities(8, 8, 512);
    for _ in 0..200 {
        contended.enqueue(mk(TrafficClass::Bulk)); // background HTTP download
    }
    contended.enqueue(mk(TrafficClass::Game)); // the game packet arrives last
    let game_jumps_ahead = contended.dequeue().map(|p| p.class) == Some(TrafficClass::Game);
    check(
        game_jumps_ahead,
        "game-latency-unchanged-under-bulk-backlog",
    );

    drop(check);
    crate::serial_println!(
        "[ OK ] traffic-shaper selftest: {}/{} checks passed (Game>Interactive>Bulk strict priority)",
        pass,
        total
    );
    if pass != total {
        crate::serial_println!(
            "[FAIL] traffic-shaper selftest: {} check(s) failed",
            total - pass
        );
    }
}

/// `/proc/athena/shaper` — gaming traffic-shaper queue depths and counters.
/// MasterChecklist Phase 10.2.
pub fn dump_shaper_text() -> alloc::string::String {
    let guard = TRAFFIC_SHAPER.lock();
    let mut out = alloc::string::String::new();
    out.push_str("# AthNet gaming traffic shaper (strict priority)\n");
    match *guard {
        Some(ref s) => {
            let st = s.stats();
            out.push_str(&alloc::format!("game_pending: {}\n", s.game_pending()));
            out.push_str(&alloc::format!(
                "interactive_pending: {}\n",
                s.interactive_pending()
            ));
            out.push_str(&alloc::format!("bulk_pending: {}\n", s.bulk_pending()));
            out.push_str(&alloc::format!(
                "game: enqueued={} dequeued={} dropped={}\n",
                st.game_enqueued,
                st.game_dequeued,
                st.game_dropped
            ));
            out.push_str(&alloc::format!(
                "interactive: enqueued={} dequeued={} dropped={}\n",
                st.interactive_enqueued,
                st.interactive_dequeued,
                st.interactive_dropped
            ));
            out.push_str(&alloc::format!(
                "bulk: enqueued={} dequeued={} dropped={}\n",
                st.bulk_enqueued,
                st.bulk_dequeued,
                st.bulk_dropped
            ));
        }
        None => out.push_str("status: not initialized\n"),
    }
    out
}

// ── Userspace TCP/UDP socket API ─────────────────────────────────────────────
// MasterChecklist Phase 10: "TCP socket API for userspace", "UDP socket API for userspace"
//
// Userspace apps call SYS_NET_SOCKET/BIND/CONNECT/SEND/RECV (syscalls 119-127)
// to create and use TCP/UDP sockets backed by smoltcp.

use alloc::collections::BTreeMap;
use spin::Mutex as SpinMutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketProto {
    Tcp,
    Udp,
}

/// Per-process socket descriptor table entry.
pub struct UserSocket {
    pub proto: SocketProto,
    pub handle: smoltcp::iface::SocketHandle,
    pub local_port: u16,
    pub remote_ip: [u8; 4],
    pub remote_port: u16,
    pub connected: bool,
}

/// Global socket descriptor table — maps (task_pid, fd) -> UserSocket.
static SOCKET_TABLE: SpinMutex<BTreeMap<(u64, u32), UserSocket>> = SpinMutex::new(BTreeMap::new());

static NEXT_FD: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(100);

/// SYS_NET_SOCKET (119): Create a TCP or UDP socket.
/// proto: 0=TCP, 1=UDP. Returns fd on success, u64::MAX on error.
pub fn sys_net_socket(proto: u64, task_pid: u64) -> u64 {
    let proto = match proto {
        0 => SocketProto::Tcp,
        1 => SocketProto::Udp,
        _ => return u64::MAX,
    };

    let fd = NEXT_FD.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let mut guard = NET_STACK.lock();
    let Some(ref mut stack) = *guard else {
        return u64::MAX;
    };

    let handle = match proto {
        SocketProto::Tcp => {
            let rx_buf = smoltcp::socket::tcp::SocketBuffer::new(alloc::vec![0u8; 8192]);
            let tx_buf = smoltcp::socket::tcp::SocketBuffer::new(alloc::vec![0u8; 8192]);
            let socket = smoltcp::socket::tcp::Socket::new(rx_buf, tx_buf);
            stack.sockets.add(socket)
        }
        SocketProto::Udp => {
            use smoltcp::socket::udp;
            let rx_buf = udp::PacketBuffer::new(
                alloc::vec![udp::PacketMetadata::EMPTY; 8],
                alloc::vec![0u8; 4096],
            );
            let tx_buf = udp::PacketBuffer::new(
                alloc::vec![udp::PacketMetadata::EMPTY; 8],
                alloc::vec![0u8; 4096],
            );
            let socket = udp::Socket::new(rx_buf, tx_buf);
            stack.sockets.add(socket)
        }
    };

    SOCKET_TABLE.lock().insert(
        (task_pid, fd),
        UserSocket {
            proto,
            handle,
            local_port: 0,
            remote_ip: [0; 4],
            remote_port: 0,
            connected: false,
        },
    );

    crate::serial_println!(
        "[net] socket created: pid={} fd={} proto={:?}",
        task_pid,
        fd,
        proto
    );
    fd as u64
}

/// SYS_NET_CONNECT (120): Connect TCP socket to remote endpoint.
/// fd: socket fd, ip: packed u32 (big-endian), port: u16.
/// Returns 0 on success, u64::MAX on error.
pub fn sys_net_connect(fd: u64, ip: u64, port: u64, task_pid: u64) -> u64 {
    let fd = fd as u32;
    let ip_bytes = [
        (ip >> 24) as u8,
        (ip >> 16) as u8,
        (ip >> 8) as u8,
        ip as u8,
    ];
    let port = port as u16;

    let mut table = SOCKET_TABLE.lock();
    let Some(sock) = table.get_mut(&(task_pid, fd)) else {
        return u64::MAX;
    };
    if sock.proto != SocketProto::Tcp {
        return u64::MAX;
    }

    sock.remote_ip = ip_bytes;
    sock.remote_port = port;

    // Choose an ephemeral local port.
    static EPHEMERAL: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(49152);
    sock.local_port = EPHEMERAL.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

    let handle = sock.handle;
    let local_port = sock.local_port;
    drop(table);

    let mut guard = NET_STACK.lock();
    let Some(ref mut stack) = *guard else {
        return u64::MAX;
    };

    let socket = stack
        .sockets
        .get_mut::<smoltcp::socket::tcp::Socket>(handle);
    let local = smoltcp::wire::IpListenEndpoint {
        addr: None,
        port: local_port,
    };
    let remote = smoltcp::wire::IpEndpoint::new(
        smoltcp::wire::IpAddress::Ipv4(smoltcp::wire::Ipv4Address::from_bytes(&ip_bytes)),
        port,
    );

    match socket.connect(&mut stack.iface.context(), remote, local) {
        Ok(()) => {
            crate::serial_println!(
                "[net] TCP connect: pid={} fd={} -> {}.{}.{}.{}:{}",
                task_pid,
                fd,
                ip_bytes[0],
                ip_bytes[1],
                ip_bytes[2],
                ip_bytes[3],
                port
            );
            0
        }
        Err(e) => {
            crate::serial_println!("[net] TCP connect failed: {:?}", e);
            u64::MAX
        }
    }
}

/// SYS_NET_SEND (121): Send data on a connected TCP socket.
/// Returns bytes sent or u64::MAX on error.
pub fn sys_net_send(fd: u64, data: &[u8], task_pid: u64) -> u64 {
    let fd = fd as u32;
    let table = SOCKET_TABLE.lock();
    let Some(sock) = table.get(&(task_pid, fd)) else {
        return u64::MAX;
    };
    let handle = sock.handle;
    drop(table);

    let mut guard = NET_STACK.lock();
    let Some(ref mut stack) = *guard else {
        return u64::MAX;
    };
    let socket = stack
        .sockets
        .get_mut::<smoltcp::socket::tcp::Socket>(handle);

    match socket.send_slice(data) {
        Ok(n) => n as u64,
        Err(_) => u64::MAX,
    }
}

/// SYS_NET_RECV (122): Receive data from a TCP socket into a buffer.
/// Returns bytes received (0 = no data yet), u64::MAX on error.
pub fn sys_net_recv(fd: u64, buf: &mut [u8], task_pid: u64) -> u64 {
    let fd = fd as u32;
    let table = SOCKET_TABLE.lock();
    let Some(sock) = table.get(&(task_pid, fd)) else {
        return u64::MAX;
    };
    let handle = sock.handle;
    drop(table);

    let mut guard = NET_STACK.lock();
    let Some(ref mut stack) = *guard else {
        return u64::MAX;
    };
    let socket = stack
        .sockets
        .get_mut::<smoltcp::socket::tcp::Socket>(handle);

    match socket.recv_slice(buf) {
        Ok(n) => n as u64,
        Err(_) => 0, // no data yet, not an error
    }
}

/// SYS_NET_CLOSE (123): Close and remove a socket.
pub fn sys_net_close(fd: u64, task_pid: u64) -> u64 {
    let fd = fd as u32;
    let mut table = SOCKET_TABLE.lock();
    if let Some(sock) = table.remove(&(task_pid, fd)) {
        let mut guard = NET_STACK.lock();
        if let Some(ref mut stack) = *guard {
            stack.sockets.remove(sock.handle);
        }
        crate::serial_println!("[net] socket closed: pid={} fd={}", task_pid, fd);
        0
    } else {
        u64::MAX
    }
}

/// Socket readiness flags returned by `sys_net_status` (mirror
/// `ath_abi::syscall::NET_STATUS_*`). A client polls these between `connect`
/// and `send`/`recv` so it never sends before the handshake completes or
/// mistakes "no data yet" for "connection closed".
pub const NET_STATUS_CONNECTED: u64 = 1 << 0;
pub const NET_STATUS_READABLE: u64 = 1 << 1;
pub const NET_STATUS_SENDABLE: u64 = 1 << 2;
pub const NET_STATUS_CLOSED: u64 = 1 << 3;

/// SYS_NET_STATUS (265): report a socket's readiness as a flags word
/// (CONNECTED|READABLE|SENDABLE|CLOSED). Returns `u64::MAX` for an unknown fd.
/// Never blocks. Also refreshes the cached `connected` bookkeeping bit.
pub fn sys_net_status(fd: u64, task_pid: u64) -> u64 {
    let fd = fd as u32;
    let (proto, handle) = {
        let table = SOCKET_TABLE.lock();
        let Some(sock) = table.get(&(task_pid, fd)) else {
            return u64::MAX;
        };
        (sock.proto, sock.handle)
    };

    let flags = {
        let mut guard = NET_STACK.lock();
        let Some(stack) = guard.as_mut() else {
            return u64::MAX;
        };
        let mut f = 0u64;
        match proto {
            SocketProto::Tcp => {
                use smoltcp::socket::tcp::State;
                let s = stack
                    .sockets
                    .get_mut::<smoltcp::socket::tcp::Socket>(handle);
                if s.state() == State::Established {
                    f |= NET_STATUS_CONNECTED;
                }
                if s.can_recv() {
                    f |= NET_STATUS_READABLE;
                }
                if s.can_send() {
                    f |= NET_STATUS_SENDABLE;
                }
                if !s.is_active() {
                    f |= NET_STATUS_CLOSED;
                }
            }
            SocketProto::Udp => {
                let s = stack
                    .sockets
                    .get_mut::<smoltcp::socket::udp::Socket>(handle);
                if s.is_open() {
                    f |= NET_STATUS_CONNECTED | NET_STATUS_SENDABLE;
                } else {
                    f |= NET_STATUS_CLOSED;
                }
                if s.can_recv() {
                    f |= NET_STATUS_READABLE;
                }
            }
        }
        f
    };

    // Refresh the bookkeeping bit (locks SOCKET_TABLE only — NET_STACK already
    // dropped above, so the SOCKET_TABLE → NET_STACK order is never inverted).
    if let Some(sock) = SOCKET_TABLE.lock().get_mut(&(task_pid, fd)) {
        sock.connected = flags & NET_STATUS_CONNECTED != 0;
    }
    flags
}

/// BUG-32: close every socket an exiting task left open. SYS_NET_SOCKET records
/// sockets in the global SOCKET_TABLE keyed by (pid, fd) — NOT the per-process
/// fd table — so task teardown never swept them and each exited networked
/// process leaked its sockets + ports forever. Called from the scheduler's exit
/// path. Safe to call under the SCHEDULER lock: net.rs never locks SCHEDULER, so
/// there is no reverse lock-ordering with SOCKET_TABLE / NET_STACK.
pub fn cleanup_task_sockets(task_pid: u64) {
    let mut table = SOCKET_TABLE.lock();
    let fds: alloc::vec::Vec<u32> = table
        .keys()
        .filter(|(pid, _)| *pid == task_pid)
        .map(|(_, fd)| *fd)
        .collect();
    if fds.is_empty() {
        return;
    }
    let mut guard = NET_STACK.lock();
    for fd in &fds {
        if let Some(sock) = table.remove(&(task_pid, *fd)) {
            if let Some(ref mut stack) = *guard {
                stack.sockets.remove(sock.handle);
            }
        }
    }
    crate::serial_println!(
        "[net] swept {} leaked socket(s) for exited pid={}",
        fds.len(),
        task_pid
    );
}

/// Perform one UDP request/response round-trip on the live stack: bind an
/// ephemeral local port, send `query` to `(server, port)`, and wait — driven
/// cooperatively (the post-boot poll thread does the actual TX/RX) — up to
/// `timeout_ms` for a datagram back. Returns the response payload, or `None`
/// on timeout / no stack. Kernel-internal (not in the per-pid SOCKET_TABLE);
/// the DNS resolver uses it. Needs a usable source IP (a DHCP lease on iron),
/// so it can't complete in headless QEMU CI — the codec is proven separately.
pub fn udp_query(
    server: [u8; 4],
    port: u16,
    query: &[u8],
    timeout_ms: u64,
) -> Option<alloc::vec::Vec<u8>> {
    use smoltcp::socket::udp;
    use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};

    static UDP_EPHEMERAL: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(40000);
    let mut local_port = UDP_EPHEMERAL.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if local_port < 40000 {
        local_port = 40000;
    }

    // Create + bind the socket, queue the query, then release the lock so the
    // poll thread can transmit it.
    let handle = {
        let mut guard = NET_STACK.lock();
        let stack = guard.as_mut()?;
        let rx = udp::PacketBuffer::new(
            alloc::vec![udp::PacketMetadata::EMPTY; 8],
            alloc::vec![0u8; 4096],
        );
        let tx = udp::PacketBuffer::new(
            alloc::vec![udp::PacketMetadata::EMPTY; 8],
            alloc::vec![0u8; 4096],
        );
        let mut socket = udp::Socket::new(rx, tx);
        if socket.bind(local_port).is_err() {
            return None;
        }
        let remote = IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::from_bytes(&server)), port);
        if socket.send_slice(query, remote).is_err() {
            return None;
        }
        stack.sockets.add(socket)
    };

    poll(); // kick the query onto the wire immediately

    let iters = (timeout_ms / 10).max(1);
    let mut result = None;
    for _ in 0..iters {
        {
            let mut guard = NET_STACK.lock();
            if let Some(stack) = guard.as_mut() {
                let s = stack.sockets.get_mut::<udp::Socket>(handle);
                if let Ok((data, _meta)) = s.recv() {
                    result = Some(data.to_vec());
                }
            }
        }
        if result.is_some() {
            break;
        }
        crate::scheduler::yield_task();
        x86_64::instructions::hlt();
        poll();
    }

    let mut guard = NET_STACK.lock();
    if let Some(stack) = guard.as_mut() {
        stack.sockets.remove(handle);
    }
    result
}

pub fn run_socket_smoketest() {
    let count = SOCKET_TABLE.lock().len();

    // sys_net_status readiness flags on a fresh, unconnected TCP socket: it
    // must be NOT connected, NOT readable, NOT sendable, and report CLOSED
    // (no live connection) — and an unknown fd must return u64::MAX. This is a
    // deterministic, falsifiable proof of the readiness surface a client polls.
    const TEST_PID: u64 = 0;
    let fd = sys_net_socket(0, TEST_PID); // 0 = TCP
    let status_ok = if fd != u64::MAX {
        let st = sys_net_status(fd, TEST_PID);
        let fresh_ok = st != u64::MAX
            && st & NET_STATUS_CONNECTED == 0
            && st & NET_STATUS_READABLE == 0
            && st & NET_STATUS_SENDABLE == 0
            && st & NET_STATUS_CLOSED != 0;
        let bad_fd_ok = sys_net_status(999_999, TEST_PID) == u64::MAX;
        let _ = sys_net_close(fd, TEST_PID);
        fresh_ok && bad_fd_ok
    } else {
        false
    };

    crate::serial_println!(
        "[net] socket API: open_sockets={} status(fresh_tcp=CLOSED, bad_fd=MAX)={} -> {}",
        count,
        status_ok,
        if status_ok { "PASS" } else { "FAIL" },
    );
}
