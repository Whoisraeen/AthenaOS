//! Netlog — UDP-broadcast the boot log over the LAN (Concept §RaeNet:
//! first-class diagnostics; a gaming OS must be debuggable on real hardware
//! without a serial cable).
//!
//! # Why this exists
//!
//! The bare-metal evidence loop (MasterChecklist Phase 1.7/1.8) depended on
//! either photographing the screen or sneakernetting a USB stick whose own
//! enumeration is one of the things under test — when USB-MSC fails, the
//! BOOTLOG.TXT channel fails WITH it. The NIC link, however, is up on Athena
//! (RTL8125 → `dhcp! net0: link up`). This module broadcasts the in-RAM
//! bootlog ring as UDP datagrams that any machine on the same L2 segment can
//! capture (`scripts/netlog-listen.ps1`) — no DHCP lease required (frames go
//! to 255.255.255.255 from 0.0.0.0, exactly like DHCPDISCOVER itself), no
//! storage writes (safe-mode compatible by construction), no USB.
//!
//! # Wire format
//!
//! Ethernet(dst=ff:ff:ff:ff:ff:ff) / IPv4(0.0.0.0 → 255.255.255.255, DF=0,
//! TTL=64) / UDP(51514 → 51514) / payload:
//!
//! ```text
//!   0..4   magic  "RLG1"
//!   4..8   boot_id  (u32 LE — TSC-derived; lets the listener separate boots)
//!   8..10  seq      (u16 LE — chunk index)
//!  10..12  total    (u16 LE — chunk count for this snapshot)
//!  12..    chunk bytes (≤ 1024)
//! ```
//!
//! The full ring snapshot is re-sent on each call (end-of-boot sends twice);
//! the listener reassembles by (boot_id, seq) and last-write-wins, so lost
//! frames only leave holes if they're lost in EVERY pass.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// UDP port (src and dst) for netlog datagrams.
const NETLOG_PORT: u16 = 51514;
/// Payload bytes per chunk. 1024 + 54 bytes of headers stays far under the
/// 1500-byte MTU every driver here supports.
const CHUNK_BYTES: usize = 1024;
/// Pace: brief pause every this many frames so a small TX ring (256 entries
/// on e1000/r8169) is never overrun by a burst.
const BURST: usize = 16;
const BURST_PAUSE_US: u64 = 2_000;

static BOOT_ID: AtomicU32 = AtomicU32::new(0);
static FRAMES_SENT: AtomicU64 = AtomicU64::new(0);
static SNAPSHOTS_SENT: AtomicUsize = AtomicUsize::new(0);
static LAST_SNAPSHOT_BYTES: AtomicUsize = AtomicUsize::new(0);

/// The MAC the frames carry as source. Real driver MAC when one is bound;
/// a locally-administered placeholder otherwise (frames are still valid —
/// the listener keys on the UDP payload, not the source).
fn source_mac() -> [u8; 6] {
    {
        let guard = crate::net_drivers::NET_DRIVERS.lock();
        if let Some(mgr) = guard.as_ref() {
            if let Some(drv) = mgr.default_driver() {
                return drv.mac_address();
            }
        }
    }
    if let Some(net) = crate::virtio_net::VIRTIO_NET.get() {
        return net.mac();
    }
    [0x02, 0x52, 0x41, 0x45, 0x45, 0x4E] // locally administered, "RAEEN"
}

/// RFC 1071 checksum over the 20-byte IPv4 header.
fn ipv4_checksum(header: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < header.len() {
        sum += u32::from(u16::from_be_bytes([header[i], header[i + 1]]));
        i += 2;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// Build one broadcast frame around `payload`.
fn build_frame(src_mac: [u8; 6], payload: &[u8]) -> Vec<u8> {
    let udp_len = 8 + payload.len();
    let ip_len = 20 + udp_len;
    let mut f = Vec::with_capacity(14 + ip_len);

    // Ethernet
    f.extend_from_slice(&[0xFF; 6]); // dst: broadcast
    f.extend_from_slice(&src_mac);
    f.extend_from_slice(&[0x08, 0x00]); // IPv4

    // IPv4 header (checksum patched after)
    let ip_start = f.len();
    f.extend_from_slice(&[0x45, 0x00]); // ver/ihl, dscp
    f.extend_from_slice(&(ip_len as u16).to_be_bytes());
    f.extend_from_slice(&FRAMES_SENT.load(Ordering::Relaxed).to_be_bytes()[6..8]); // id
    f.extend_from_slice(&[0x00, 0x00]); // flags/frag
    f.extend_from_slice(&[64, 17]); // ttl, proto=UDP
    f.extend_from_slice(&[0x00, 0x00]); // checksum placeholder
    f.extend_from_slice(&[0, 0, 0, 0]); // src 0.0.0.0
    f.extend_from_slice(&[255, 255, 255, 255]); // dst broadcast
    let csum = ipv4_checksum(&f[ip_start..ip_start + 20]);
    f[ip_start + 10..ip_start + 12].copy_from_slice(&csum.to_be_bytes());

    // UDP (checksum 0 = "not computed", legal for IPv4)
    f.extend_from_slice(&NETLOG_PORT.to_be_bytes());
    f.extend_from_slice(&NETLOG_PORT.to_be_bytes());
    f.extend_from_slice(&(udp_len as u16).to_be_bytes());
    f.extend_from_slice(&[0x00, 0x00]);

    f.extend_from_slice(payload);
    f
}

/// Broadcast the current bootlog ring as a chunked snapshot. `tag` names the
/// call site in the proof line (which itself lands in the ring for the NEXT
/// snapshot/photo). Returns the number of chunks sent.
pub fn broadcast_ring(tag: &str) -> usize {
    let snapshot = crate::bootlog::snapshot();
    let bytes = snapshot.as_bytes();
    if bytes.is_empty() {
        return 0;
    }
    let total = (bytes.len() + CHUNK_BYTES - 1) / CHUNK_BYTES;
    // u16 seq space caps the snapshot at 64 MiB — the ring is 1 MiB.
    let total_u16 = total.min(u16::MAX as usize) as u16;
    let boot_id = BOOT_ID.load(Ordering::Relaxed);
    let src_mac = source_mac();

    let mut sent = 0usize;
    for (seq, chunk) in bytes
        .chunks(CHUNK_BYTES)
        .enumerate()
        .take(total_u16 as usize)
    {
        let mut payload = Vec::with_capacity(12 + chunk.len());
        payload.extend_from_slice(b"RLG1");
        payload.extend_from_slice(&boot_id.to_le_bytes());
        payload.extend_from_slice(&(seq as u16).to_le_bytes());
        payload.extend_from_slice(&total_u16.to_le_bytes());
        payload.extend_from_slice(chunk);

        crate::net::transmit_raw(&build_frame(src_mac, &payload));
        sent += 1;
        FRAMES_SENT.fetch_add(1, Ordering::Relaxed);
        if sent % BURST == 0 {
            // Let the TX ring drain; wall-clock, immune to TSC calibration.
            let _ = crate::hpet::spin_until_us(BURST_PAUSE_US, || false);
        }
    }

    SNAPSHOTS_SENT.fetch_add(1, Ordering::Relaxed);
    LAST_SNAPSHOT_BYTES.store(bytes.len(), Ordering::Relaxed);
    crate::serial_println!(
        "[netlog] {}: broadcast {} chunk(s) / {} bytes on UDP {} (boot_id={:#010x})",
        tag,
        sent,
        bytes.len(),
        NETLOG_PORT,
        boot_id,
    );
    sent
}

/// Initialize: derive a boot id so the listener can tell boots apart.
pub fn init() {
    let tsc = unsafe { core::arch::x86_64::_rdtsc() };
    let id = (tsc as u32) ^ ((tsc >> 32) as u32) | 1;
    BOOT_ID.store(id, Ordering::Relaxed);
    crate::serial_println!(
        "[netlog] initialized: UDP-broadcast log channel on port {} (boot_id={:#010x})",
        NETLOG_PORT,
        id,
    );
}

/// R10 smoketest: send a single tiny snapshot ("netlog-smoketest" chunk) and
/// prove the frame builder + TX path accept it. Deterministic — works with
/// QEMU's e1000/virtio user-net (frames are accepted regardless of listener).
pub fn run_boot_smoketest() {
    let payload = b"RLG1\x00\x00\x00\x00\x00\x00\x01\x00netlog-smoketest";
    let frame = build_frame(source_mac(), payload);
    // Validate our own header arithmetic before handing it to a NIC.
    let ip_ok = frame.len() == 14 + 20 + 8 + payload.len()
        && frame[12] == 0x08
        && frame[23] == 17
        && ipv4_checksum(&frame[14..34]) == 0; // checksum over a checksummed header folds to 0
    crate::net::transmit_raw(&frame);
    FRAMES_SENT.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!(
        "[netlog] smoketest: frame={}B ip_header={} tx=submitted -> {}",
        frame.len(),
        if ip_ok { "valid" } else { "INVALID" },
        if ip_ok { "PASS" } else { "FAIL" },
    );
}

/// `/proc/raeen/netlog` — counters for the broadcast log channel.
pub fn dump_text() -> String {
    alloc::format!(
        "# netlog (UDP-broadcast bootlog, port {})\nboot_id: {:#010x}\nframes_sent: {}\nsnapshots_sent: {}\nlast_snapshot_bytes: {}\n",
        NETLOG_PORT,
        BOOT_ID.load(Ordering::Relaxed),
        FRAMES_SENT.load(Ordering::Relaxed),
        SNAPSHOTS_SENT.load(Ordering::Relaxed),
        LAST_SNAPSHOT_BYTES.load(Ordering::Relaxed),
    )
}
