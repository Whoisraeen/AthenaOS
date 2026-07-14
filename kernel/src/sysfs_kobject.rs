//! /sys virtual filesystem (sysfs) for Linux compatibility.
//!
//! Exposes hardware and kernel state through a hierarchical virtual filesystem,
//! matching the Linux sysfs layout that userspace tools expect. Covers:
//!
//! - `/sys/class/net/[iface]/` — network interface attributes
//! - `/sys/class/block/[dev]/` — block device attributes
//! - `/sys/devices/system/cpu/cpu[n]/` — per-CPU info, topology, cpufreq
//! - `/sys/devices/system/memory/` — memory block info
//! - `/sys/kernel/` — hostname, ostype, osrelease, version
//! - `/sys/fs/` — special filesystem mount points

#![allow(dead_code)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

// ═══════════════════════════════════════════════════════════════════════════════
// Kobject: the base abstraction for sysfs entries
// ═══════════════════════════════════════════════════════════════════════════════

pub struct KObject {
    pub name: String,
    pub kobj_type: KObjectType,
}

#[derive(Clone, Debug, PartialEq)]
pub enum KObjectType {
    Directory,
    Attribute,
    Symlink(String),
}

impl KObject {
    pub fn dir(name: &str) -> Self {
        Self {
            name: String::from(name),
            kobj_type: KObjectType::Directory,
        }
    }
    pub fn attr(name: &str) -> Self {
        Self {
            name: String::from(name),
            kobj_type: KObjectType::Attribute,
        }
    }
    pub fn symlink(name: &str, target: &str) -> Self {
        Self {
            name: String::from(name),
            kobj_type: KObjectType::Symlink(String::from(target)),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// /sys/kernel — kernel-level attributes
// ═══════════════════════════════════════════════════════════════════════════════

fn sys_kernel_hostname() -> String {
    String::from("athenaos\n")
}

fn sys_kernel_ostype() -> String {
    String::from("AthenaOS\n")
}

fn sys_kernel_osrelease() -> String {
    String::from("0.0.1-athenaos\n")
}

fn sys_kernel_version() -> String {
    String::from("#1 SMP AthenaOS 0.0.1\n")
}

fn sys_kernel_domainname() -> String {
    String::from("(none)\n")
}

// ═══════════════════════════════════════════════════════════════════════════════
// /sys/class/net/[iface]/ — network interface attributes
// ═══════════════════════════════════════════════════════════════════════════════

pub struct NetIfaceInfo {
    pub name: String,
    pub mac: [u8; 6],
    pub mtu: u32,
    pub speed: u32,
    pub operstate: NetOperState,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub tx_packets: u64,
    pub rx_packets: u64,
    pub tx_errors: u64,
    pub rx_errors: u64,
    pub tx_dropped: u64,
    pub rx_dropped: u64,
    pub carrier: bool,
    pub flags: u32,
    pub ifindex: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NetOperState {
    Up,
    Down,
    Unknown,
    LowerLayerDown,
    Testing,
    Dormant,
    NotPresent,
}

impl NetOperState {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::Unknown => "unknown",
            Self::LowerLayerDown => "lowerlayerdown",
            Self::Testing => "testing",
            Self::Dormant => "dormant",
            Self::NotPresent => "notpresent",
        }
    }
}

impl NetIfaceInfo {
    pub fn loopback() -> Self {
        Self {
            name: String::from("lo"),
            mac: [0, 0, 0, 0, 0, 0],
            mtu: 65536,
            speed: 0,
            operstate: NetOperState::Unknown,
            tx_bytes: 0,
            rx_bytes: 0,
            tx_packets: 0,
            rx_packets: 0,
            tx_errors: 0,
            rx_errors: 0,
            tx_dropped: 0,
            rx_dropped: 0,
            carrier: true,
            flags: 0x49, // IFF_UP | IFF_LOOPBACK | IFF_RUNNING
            ifindex: 1,
        }
    }

    pub fn eth0() -> Self {
        Self {
            name: String::from("eth0"),
            mac: [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
            mtu: 1500,
            speed: 1000,
            operstate: NetOperState::Up,
            tx_bytes: 0,
            rx_bytes: 0,
            tx_packets: 0,
            rx_packets: 0,
            tx_errors: 0,
            rx_errors: 0,
            tx_dropped: 0,
            rx_dropped: 0,
            carrier: true,
            flags: 0x1003, // IFF_UP | IFF_BROADCAST | IFF_MULTICAST
            ifindex: 2,
        }
    }

    fn format_mac(&self) -> String {
        format!(
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.mac[0], self.mac[1], self.mac[2], self.mac[3], self.mac[4], self.mac[5],
        )
    }
}

fn resolve_net_iface(iface: &str, attr: &str) -> Option<String> {
    let info = match iface {
        "lo" => NetIfaceInfo::loopback(),
        "eth0" => NetIfaceInfo::eth0(),
        _ => return None,
    };

    match attr {
        "speed" => Some(format!("{}\n", info.speed)),
        "mtu" => Some(format!("{}\n", info.mtu)),
        "address" => Some(format!("{}\n", info.format_mac())),
        "operstate" => Some(format!("{}\n", info.operstate.as_str())),
        "carrier" => Some(format!("{}\n", if info.carrier { 1 } else { 0 })),
        "flags" => Some(format!("0x{:x}\n", info.flags)),
        "ifindex" => Some(format!("{}\n", info.ifindex)),
        "type" => Some(String::from("1\n")),
        "tx_queue_len" => Some(String::from("1000\n")),
        "addr_len" => Some(String::from("6\n")),
        "dormant" => Some(String::from("0\n")),
        "broadcast" => Some(String::from("ff:ff:ff:ff:ff:ff\n")),
        "link_mode" => Some(String::from("0\n")),
        "netdev_group" => Some(String::from("0\n")),
        "proto_down" => Some(String::from("0\n")),
        _ => {
            if let Some(stat) = attr.strip_prefix("statistics/") {
                resolve_net_statistics(&info, stat)
            } else {
                None
            }
        }
    }
}

fn resolve_net_statistics(info: &NetIfaceInfo, stat: &str) -> Option<String> {
    match stat {
        "tx_bytes" => Some(format!("{}\n", info.tx_bytes)),
        "rx_bytes" => Some(format!("{}\n", info.rx_bytes)),
        "tx_packets" => Some(format!("{}\n", info.tx_packets)),
        "rx_packets" => Some(format!("{}\n", info.rx_packets)),
        "tx_errors" => Some(format!("{}\n", info.tx_errors)),
        "rx_errors" => Some(format!("{}\n", info.rx_errors)),
        "tx_dropped" => Some(format!("{}\n", info.tx_dropped)),
        "rx_dropped" => Some(format!("{}\n", info.rx_dropped)),
        "collisions" => Some(String::from("0\n")),
        "multicast" => Some(String::from("0\n")),
        "rx_compressed" => Some(String::from("0\n")),
        "tx_compressed" => Some(String::from("0\n")),
        "rx_crc_errors" => Some(String::from("0\n")),
        "rx_fifo_errors" => Some(String::from("0\n")),
        "rx_frame_errors" => Some(String::from("0\n")),
        "rx_length_errors" => Some(String::from("0\n")),
        "rx_missed_errors" => Some(String::from("0\n")),
        "rx_over_errors" => Some(String::from("0\n")),
        "tx_aborted_errors" => Some(String::from("0\n")),
        "tx_carrier_errors" => Some(String::from("0\n")),
        "tx_fifo_errors" => Some(String::from("0\n")),
        "tx_heartbeat_errors" => Some(String::from("0\n")),
        "tx_window_errors" => Some(String::from("0\n")),
        _ => None,
    }
}

fn list_net_iface(iface: &str) -> Option<String> {
    match iface {
        "lo" | "eth0" => Some(String::from(
            "speed\nmtu\naddress\noperstate\ncarrier\nflags\n\
             ifindex\ntype\ntx_queue_len\naddr_len\nbroadcast\n\
             dormant\nlink_mode\nnetdev_group\nproto_down\nstatistics\n",
        )),
        _ => None,
    }
}

fn list_net_interfaces() -> String {
    String::from("lo\neth0\n")
}

fn list_net_statistics() -> String {
    String::from(
        "tx_bytes\nrx_bytes\ntx_packets\nrx_packets\n\
         tx_errors\nrx_errors\ntx_dropped\nrx_dropped\n\
         collisions\nmulticast\n",
    )
}

// ═══════════════════════════════════════════════════════════════════════════════
// /sys/class/block/[dev]/ — block device attributes
// ═══════════════════════════════════════════════════════════════════════════════

pub struct BlockDevInfo {
    pub name: String,
    pub size_sectors: u64,
    pub removable: bool,
    pub readonly: bool,
    pub alignment_offset: u32,
    pub discard_alignment: u32,
    pub hw_sector_size: u32,
    pub logical_block_size: u32,
    pub physical_block_size: u32,
    pub min_io_size: u32,
    pub opt_io_size: u32,
    pub nr_requests: u32,
    pub scheduler: String,
}

impl BlockDevInfo {
    pub fn nvme0n1() -> Self {
        Self {
            name: String::from("nvme0n1"),
            size_sectors: 2097152, // ~1GB
            removable: false,
            readonly: false,
            alignment_offset: 0,
            discard_alignment: 0,
            hw_sector_size: 512,
            logical_block_size: 512,
            physical_block_size: 512,
            min_io_size: 512,
            opt_io_size: 0,
            nr_requests: 1024,
            scheduler: String::from("[none] mq-deadline"),
        }
    }

    pub fn sda() -> Self {
        Self {
            name: String::from("sda"),
            size_sectors: 4194304, // ~2GB
            removable: false,
            readonly: false,
            alignment_offset: 0,
            discard_alignment: 0,
            hw_sector_size: 512,
            logical_block_size: 512,
            physical_block_size: 512,
            min_io_size: 512,
            opt_io_size: 0,
            nr_requests: 128,
            scheduler: String::from("[mq-deadline] none"),
        }
    }
}

fn resolve_block_dev(dev: &str, attr: &str) -> Option<String> {
    let info = match dev {
        "nvme0n1" => BlockDevInfo::nvme0n1(),
        "sda" => BlockDevInfo::sda(),
        _ => return None,
    };

    match attr {
        "size" => Some(format!("{}\n", info.size_sectors)),
        "removable" => Some(format!("{}\n", if info.removable { 1 } else { 0 })),
        "ro" => Some(format!("{}\n", if info.readonly { 1 } else { 0 })),
        "alignment_offset" => Some(format!("{}\n", info.alignment_offset)),
        "discard_alignment" => Some(format!("{}\n", info.discard_alignment)),
        "stat" => Some(String::from(
            "    0     0     0     0     0     0     0     0     0     0     0\n",
        )),
        "dev" => Some(format!("{}:{}\n", 259, 0)),
        "range" => Some(String::from("0\n")),
        "ext_range" => Some(String::from("256\n")),
        "events" => Some(String::from("\n")),
        "events_async" => Some(String::from("\n")),
        "events_poll_msecs" => Some(String::from("-1\n")),
        "inflight" => Some(String::from("       0        0\n")),
        _ => {
            if let Some(q_attr) = attr.strip_prefix("queue/") {
                resolve_block_queue(&info, q_attr)
            } else {
                None
            }
        }
    }
}

fn resolve_block_queue(info: &BlockDevInfo, attr: &str) -> Option<String> {
    match attr {
        "hw_sector_size" => Some(format!("{}\n", info.hw_sector_size)),
        "logical_block_size" => Some(format!("{}\n", info.logical_block_size)),
        "physical_block_size" => Some(format!("{}\n", info.physical_block_size)),
        "minimum_io_size" => Some(format!("{}\n", info.min_io_size)),
        "optimal_io_size" => Some(format!("{}\n", info.opt_io_size)),
        "nr_requests" => Some(format!("{}\n", info.nr_requests)),
        "scheduler" => Some(format!("{}\n", info.scheduler)),
        "max_hw_sectors_kb" => Some(String::from("32767\n")),
        "max_sectors_kb" => Some(String::from("1280\n")),
        "read_ahead_kb" => Some(String::from("128\n")),
        "rotational" => Some(String::from("0\n")),
        "rq_affinity" => Some(String::from("2\n")),
        "add_random" => Some(String::from("0\n")),
        "discard_granularity" => Some(String::from("0\n")),
        "discard_max_bytes" => Some(String::from("0\n")),
        "discard_max_hw_bytes" => Some(String::from("0\n")),
        "discard_zeroes_data" => Some(String::from("0\n")),
        "write_cache" => Some(String::from("write back\n")),
        "nomerges" => Some(String::from("0\n")),
        "io_poll" => Some(String::from("0\n")),
        "io_poll_delay" => Some(String::from("-1\n")),
        "wbt_lat_usec" => Some(String::from("75000\n")),
        "zoned" => Some(String::from("none\n")),
        "max_open_zones" => Some(String::from("0\n")),
        "max_active_zones" => Some(String::from("0\n")),
        "chunk_sectors" => Some(String::from("0\n")),
        _ => None,
    }
}

fn list_block_dev(dev: &str) -> Option<String> {
    match dev {
        "nvme0n1" | "sda" => Some(String::from(
            "size\nremovable\nro\nalignment_offset\ndiscard_alignment\n\
             stat\ndev\nrange\next_range\nevents\nevents_async\n\
             events_poll_msecs\ninflight\nqueue\n",
        )),
        _ => None,
    }
}

fn list_block_devices() -> String {
    String::from("nvme0n1\nsda\n")
}

fn list_block_queue() -> String {
    String::from(
        "hw_sector_size\nlogical_block_size\nphysical_block_size\n\
         minimum_io_size\noptimal_io_size\nnr_requests\nscheduler\n\
         max_hw_sectors_kb\nmax_sectors_kb\nread_ahead_kb\nrotational\n\
         rq_affinity\nadd_random\nwrite_cache\nnomerges\n",
    )
}

// ═══════════════════════════════════════════════════════════════════════════════
// /sys/devices/system/cpu/cpu[n]/ — per-CPU attributes
// ═══════════════════════════════════════════════════════════════════════════════

fn resolve_cpu(cpu_id: u32, attr: &str) -> Option<String> {
    match attr {
        "online" => Some(String::from("1\n")),
        _ => {
            if let Some(topo) = attr.strip_prefix("topology/") {
                resolve_cpu_topology(cpu_id, topo)
            } else if let Some(freq) = attr.strip_prefix("cpufreq/") {
                resolve_cpu_freq(cpu_id, freq)
            } else {
                None
            }
        }
    }
}

fn resolve_cpu_topology(cpu_id: u32, attr: &str) -> Option<String> {
    match attr {
        "physical_package_id" => Some(String::from("0\n")),
        "core_id" => Some(format!("{}\n", cpu_id)),
        "thread_siblings" => Some(format!("{:x}\n", 1u32 << cpu_id)),
        "thread_siblings_list" => Some(format!("{}\n", cpu_id)),
        "core_siblings" => Some(String::from("f\n")),
        "core_siblings_list" => Some(String::from("0-3\n")),
        "die_id" => Some(String::from("0\n")),
        "cluster_id" => Some(String::from("0\n")),
        _ => None,
    }
}

fn resolve_cpu_freq(cpu_id: u32, attr: &str) -> Option<String> {
    let _cpu_id = cpu_id;
    match attr {
        "scaling_cur_freq" => Some(String::from("3000000\n")),
        "scaling_min_freq" => Some(String::from("800000\n")),
        "scaling_max_freq" => Some(String::from("3000000\n")),
        "cpuinfo_min_freq" => Some(String::from("800000\n")),
        "cpuinfo_max_freq" => Some(String::from("3000000\n")),
        "cpuinfo_cur_freq" => Some(String::from("3000000\n")),
        "scaling_governor" => Some(String::from("performance\n")),
        "scaling_available_governors" => Some(String::from("performance powersave\n")),
        "scaling_driver" => Some(String::from("raeen-cpufreq\n")),
        "energy_performance_preference" => Some(String::from("performance\n")),
        "energy_performance_available_preferences" => Some(String::from(
            "default performance balance_performance balance_power power\n",
        )),
        "related_cpus" => Some(format!("{}\n", cpu_id)),
        "affected_cpus" => Some(format!("{}\n", cpu_id)),
        _ => None,
    }
}

fn list_cpu_dir(cpu_id: u32) -> Option<String> {
    let _cpu_id = cpu_id;
    Some(String::from("online\ntopology\ncpufreq\n"))
}

fn list_cpu_topology() -> String {
    String::from(
        "physical_package_id\ncore_id\nthread_siblings\n\
         thread_siblings_list\ncore_siblings\ncore_siblings_list\n\
         die_id\ncluster_id\n",
    )
}

fn list_cpu_freq() -> String {
    String::from(
        "scaling_cur_freq\nscaling_min_freq\nscaling_max_freq\n\
         cpuinfo_min_freq\ncpuinfo_max_freq\ncpuinfo_cur_freq\n\
         scaling_governor\nscaling_available_governors\nscaling_driver\n\
         energy_performance_preference\nenergy_performance_available_preferences\n\
         related_cpus\naffected_cpus\n",
    )
}

fn list_cpus() -> String {
    String::from("cpu0\nkernel_max\noffline\nonline\npossible\npresent\n")
}

fn resolve_cpu_global(attr: &str) -> Option<String> {
    match attr {
        "kernel_max" => Some(String::from("255\n")),
        "offline" => Some(String::from("\n")),
        "online" => Some(String::from("0\n")),
        "possible" => Some(String::from("0\n")),
        "present" => Some(String::from("0\n")),
        _ => None,
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// /sys/devices/system/memory/ — memory block attributes
// ═══════════════════════════════════════════════════════════════════════════════

fn resolve_memory(attr: &str) -> Option<String> {
    match attr {
        "block_size_bytes" => Some(String::from("8000000\n")),
        "auto_online_blocks" => Some(String::from("offline\n")),
        _ => {
            if let Some(rest) = attr.strip_prefix("memory") {
                let parts: Vec<&str> = rest.splitn(2, '/').collect();
                if let Some(&block_str) = parts.first() {
                    if let Ok(_block_id) = parse_u32(block_str) {
                        if let Some(&sub_attr) = parts.get(1) {
                            return resolve_memory_block(sub_attr);
                        }
                        return Some(String::from(
                            "phys_device\nphys_index\nonline_type\nremovable\nstate\nvalid_zones\n",
                        ));
                    }
                }
                None
            } else {
                None
            }
        }
    }
}

fn resolve_memory_block(attr: &str) -> Option<String> {
    match attr {
        "phys_device" => Some(String::from("0\n")),
        "phys_index" => Some(String::from("00000000\n")),
        "online_type" => Some(String::from("online\n")),
        "removable" => Some(String::from("0\n")),
        "state" => Some(String::from("online\n")),
        "valid_zones" => Some(String::from("Normal\n")),
        _ => None,
    }
}

fn list_memory() -> String {
    String::from("block_size_bytes\nauto_online_blocks\nmemory0\n")
}

// ═══════════════════════════════════════════════════════════════════════════════
// /sys/fs/ — filesystem mount points
// ═══════════════════════════════════════════════════════════════════════════════

fn resolve_fs(attr: &str) -> Option<String> {
    match attr {
        "cgroup" => Some(String::new()),
        "pstore" => Some(String::new()),
        "bpf" => Some(String::new()),
        _ => None,
    }
}

fn list_fs() -> String {
    String::from("cgroup\npstore\nbpf\n")
}

// ═══════════════════════════════════════════════════════════════════════════════
// Master path resolver: /sys/... → content
// ═══════════════════════════════════════════════════════════════════════════════

pub fn resolve_sys_path(path: &str) -> Option<String> {
    let path = path.trim_start_matches("/sys");
    let path = path.trim_start_matches('/');

    if path.is_empty() {
        return Some(String::from(
            "class\ndevices\nkernel\nfs\nmodule\npower\nfirmware\n",
        ));
    }

    // /sys/kernel/...
    if let Some(rest) = path.strip_prefix("kernel/") {
        return resolve_kernel_attr(rest);
    }
    if path == "kernel" {
        return Some(String::from(
            "hostname\nostype\nosrelease\nversion\ndomainname\n",
        ));
    }

    // /sys/class/net/...
    if let Some(rest) = path.strip_prefix("class/net/") {
        return resolve_class_net(rest);
    }
    if path == "class/net" {
        return Some(list_net_interfaces());
    }
    if path == "class" {
        return Some(String::from("net\nblock\ntty\ninput\n"));
    }

    // /sys/class/block/...
    if let Some(rest) = path.strip_prefix("class/block/") {
        return resolve_class_block(rest);
    }
    if path == "class/block" {
        return Some(list_block_devices());
    }

    // /sys/devices/system/cpu/...
    if let Some(rest) = path.strip_prefix("devices/system/cpu/") {
        return resolve_devices_cpu(rest);
    }
    if path == "devices/system/cpu" {
        return Some(list_cpus());
    }

    // /sys/devices/system/memory/...
    if let Some(rest) = path.strip_prefix("devices/system/memory/") {
        return resolve_memory(rest);
    }
    if path == "devices/system/memory" {
        return Some(list_memory());
    }

    // /sys/devices/system
    if path == "devices/system" {
        return Some(String::from(
            "cpu\nmemory\nnode\nclockevents\nclocksource\n",
        ));
    }
    if path == "devices" {
        return Some(String::from("system\nplatform\npci0000:00\nvirtual\n"));
    }

    // /sys/fs/...
    if let Some(rest) = path.strip_prefix("fs/") {
        return resolve_fs(rest);
    }
    if path == "fs" {
        return Some(list_fs());
    }

    // /sys/module
    if path == "module" {
        return Some(String::new());
    }

    // /sys/power
    if path == "power" {
        return Some(String::from("state\nwakeup_count\npm_async\n"));
    }
    if let Some(rest) = path.strip_prefix("power/") {
        return resolve_power(rest);
    }

    // /sys/firmware
    if path == "firmware" {
        return Some(String::from("acpi\nefi\n"));
    }

    None
}

fn resolve_kernel_attr(attr: &str) -> Option<String> {
    match attr {
        "hostname" => Some(sys_kernel_hostname()),
        "ostype" => Some(sys_kernel_ostype()),
        "osrelease" => Some(sys_kernel_osrelease()),
        "version" => Some(sys_kernel_version()),
        "domainname" => Some(sys_kernel_domainname()),
        _ => None,
    }
}

fn resolve_class_net(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.splitn(2, '/').collect();
    let iface = parts[0];
    if parts.len() == 1 {
        return list_net_iface(iface);
    }
    let attr = parts[1];

    if attr == "statistics" {
        return Some(list_net_statistics());
    }

    resolve_net_iface(iface, attr)
}

fn resolve_class_block(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.splitn(2, '/').collect();
    let dev = parts[0];
    if parts.len() == 1 {
        return list_block_dev(dev);
    }
    let attr = parts[1];

    if attr == "queue" {
        return Some(list_block_queue());
    }

    resolve_block_dev(dev, attr)
}

fn resolve_devices_cpu(path: &str) -> Option<String> {
    if let Some(rest) = path.strip_prefix("cpu") {
        let parts: Vec<&str> = rest.splitn(2, '/').collect();
        if let Ok(cpu_id) = parse_u32(parts[0]) {
            if parts.len() == 1 {
                return list_cpu_dir(cpu_id);
            }
            let sub = parts[1];
            if sub == "topology" {
                return Some(list_cpu_topology());
            }
            if sub == "cpufreq" {
                return Some(list_cpu_freq());
            }
            return resolve_cpu(cpu_id, sub);
        }
    }
    resolve_cpu_global(path)
}

fn resolve_power(attr: &str) -> Option<String> {
    match attr {
        "state" => Some(String::from("mem\n")),
        "wakeup_count" => Some(String::from("0\n")),
        "pm_async" => Some(String::from("0\n")),
        _ => None,
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// VFS Inode adapter
// ═══════════════════════════════════════════════════════════════════════════════

pub struct SysfsPathInode {
    path: String,
}

impl SysfsPathInode {
    pub fn new(path: &str) -> Self {
        Self {
            path: String::from(path),
        }
    }
}

impl crate::vfs::Inode for SysfsPathInode {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        if let Some(content) = resolve_sys_path(&self.path) {
            let bytes = content.as_bytes();
            if offset >= bytes.len() {
                return 0;
            }
            let n = buf.len().min(bytes.len() - offset);
            buf[..n].copy_from_slice(&bytes[offset..offset + n]);
            n
        } else {
            0
        }
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> usize {
        0
    }
    fn size(&self) -> usize {
        resolve_sys_path(&self.path).map(|c| c.len()).unwrap_or(0)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════════════

fn parse_u32(s: &str) -> Result<u32, ()> {
    let mut result: u32 = 0;
    if s.is_empty() {
        return Err(());
    }
    for b in s.bytes() {
        if b < b'0' || b > b'9' {
            return Err(());
        }
        result = result.checked_mul(10).ok_or(())?;
        result = result.checked_add((b - b'0') as u32).ok_or(())?;
    }
    Ok(result)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Initialization
// ═══════════════════════════════════════════════════════════════════════════════

pub fn init() {
    crate::serial_println!("[ OK ] /sys filesystem (sysfs) initialized");
}
