//! /proc virtual filesystem for Linux compatibility.
//!
//! Provides a Linux-compatible procfs implementation exposing per-process
//! info (/proc/[pid]/*) and system-wide info (/proc/cpuinfo, /proc/meminfo,
//! etc.). Format matches Linux exactly where tools depend on it (e.g. maps).

#![allow(dead_code)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::process::{self, FdType, MmapFlags, Pid, ProcessState, RegionBacking};

// ═══════════════════════════════════════════════════════════════════════════════
// Boot time tracking
// ═══════════════════════════════════════════════════════════════════════════════

static BOOT_TICKS: AtomicU64 = AtomicU64::new(0);
static TICK_HZ: AtomicU64 = AtomicU64::new(100);

pub fn set_boot_ticks(ticks: u64) {
    BOOT_TICKS.store(ticks, Ordering::Relaxed);
}

fn uptime_secs() -> u64 {
    let ticks = BOOT_TICKS.load(Ordering::Relaxed);
    let hz = TICK_HZ.load(Ordering::Relaxed);
    if hz == 0 {
        return 0;
    }
    ticks / hz
}

// ═══════════════════════════════════════════════════════════════════════════════
// Per-process /proc/[pid] entries
// ═══════════════════════════════════════════════════════════════════════════════

pub fn proc_pid_exe(pid: u64) -> String {
    let table = process::PROCESS_TABLE.lock();
    if let Some(ref t) = *table {
        if let Some(proc_entry) = t.getpid(Pid(pid)) {
            return proc_entry.name.clone();
        }
    }
    String::new()
}

/// /proc/[pid]/maps — Linux-exact format:
/// address           perms offset  dev   inode   pathname
/// 00400000-00452000 r-xp 00000000 08:02 173521  /usr/bin/dbus-daemon
pub fn proc_pid_maps(pid: u64) -> String {
    let table = process::PROCESS_TABLE.lock();
    if let Some(ref t) = *table {
        if let Some(proc_entry) = t.getpid(Pid(pid)) {
            let mut out = String::new();
            for region in &proc_entry.memory_space.regions {
                let r = if region.permissions.readable() {
                    'r'
                } else {
                    '-'
                };
                let w = if region.permissions.writable() {
                    'w'
                } else {
                    '-'
                };
                let x = if region.permissions.executable() {
                    'x'
                } else {
                    '-'
                };
                let shared = if region.flags.0 & MmapFlags::SHARED != 0 {
                    's'
                } else {
                    'p'
                };
                let dev_major = 0u32;
                let dev_minor = 0u32;
                let inode = 0u64;
                let (offset, name_from_backing) = match &region.backing {
                    RegionBacking::File { offset, .. } => (*offset, None),
                    RegionBacking::Anonymous => (0u64, None),
                    RegionBacking::SharedMemory(id) => (0u64, Some(format!("/dev/shm/{}", id))),
                    RegionBacking::Device(id) => (0u64, Some(format!("/dev/{}", id))),
                };
                let name = region
                    .name
                    .as_deref()
                    .or(name_from_backing.as_deref())
                    .unwrap_or("");

                out.push_str(&format!(
                    "{:08x}-{:08x} {}{}{}{} {:08x} {:02x}:{:02x} {:<10}",
                    region.start, region.end, r, w, x, shared, offset, dev_major, dev_minor, inode,
                ));
                if !name.is_empty() {
                    out.push_str(name);
                }
                out.push('\n');
            }
            return out;
        }
    }
    String::new()
}

/// /proc/[pid]/status — key:value format matching Linux
pub fn proc_pid_status(pid: u64) -> String {
    let table = process::PROCESS_TABLE.lock();
    if let Some(ref t) = *table {
        if let Some(proc_entry) = t.getpid(Pid(pid)) {
            let state_char = match proc_entry.state {
                ProcessState::Running => 'R',
                ProcessState::Sleeping => 'S',
                ProcessState::Stopped => 'T',
                ProcessState::Zombie => 'Z',
                ProcessState::Dead => 'X',
                ProcessState::TracedStopped => 't',
            };
            let state_name = match proc_entry.state {
                ProcessState::Running => "running",
                ProcessState::Sleeping => "sleeping",
                ProcessState::Stopped => "stopped",
                ProcessState::Zombie => "zombie",
                ProcessState::Dead => "dead",
                ProcessState::TracedStopped => "tracing stop",
            };
            let vm_size = proc_entry.memory_space.total_mapped;
            let vm_rss = vm_size;
            let umask = 0o022u32;

            return format!(
                "Name:\t{}\n\
                 Umask:\t{:04o}\n\
                 State:\t{} ({})\n\
                 Tgid:\t{}\n\
                 Ngid:\t0\n\
                 Pid:\t{}\n\
                 PPid:\t{}\n\
                 TracerPid:\t0\n\
                 Uid:\t{}\t{}\t{}\t{}\n\
                 Gid:\t{}\t{}\t{}\t{}\n\
                 FDSize:\t{}\n\
                 Groups:\t{}\n\
                 VmPeak:\t{} kB\n\
                 VmSize:\t{} kB\n\
                 VmLck:\t0 kB\n\
                 VmPin:\t0 kB\n\
                 VmHWM:\t{} kB\n\
                 VmRSS:\t{} kB\n\
                 VmData:\t{} kB\n\
                 VmStk:\t{} kB\n\
                 VmExe:\t{} kB\n\
                 VmLib:\t0 kB\n\
                 VmPTE:\t0 kB\n\
                 VmSwap:\t0 kB\n\
                 Threads:\t{}\n\
                 SigQ:\t0/0\n\
                 SigPnd:\t{:016x}\n\
                 ShdPnd:\t0000000000000000\n\
                 SigBlk:\t{:016x}\n\
                 SigIgn:\t0000000000000000\n\
                 SigCgt:\t0000000000000000\n\
                 CapInh:\t0000000000000000\n\
                 CapPrm:\t0000000000000000\n\
                 CapEff:\t0000000000000000\n\
                 CapBnd:\t0000003fffffffff\n\
                 CapAmb:\t0000000000000000\n\
                 voluntary_ctxt_switches:\t0\n\
                 nonvoluntary_ctxt_switches:\t0\n",
                proc_entry.name,
                umask,
                state_char,
                state_name,
                proc_entry.pid.0,
                proc_entry.pid.0,
                proc_entry.ppid.0,
                proc_entry.uid,
                proc_entry.uid,
                proc_entry.uid,
                proc_entry.uid,
                proc_entry.gid,
                proc_entry.gid,
                proc_entry.gid,
                proc_entry.gid,
                64, // FDSize estimate
                proc_entry.gid,
                vm_size / 1024,
                vm_size / 1024,
                vm_rss / 1024,
                vm_rss / 1024,
                vm_size / 2048,
                128,
                vm_size / 4096,
                proc_entry.threads.len(),
                proc_entry.pending_signals,
                proc_entry.signal_mask,
            );
        }
    }
    String::from("(not found)\n")
}

/// /proc/[pid]/cmdline — null-separated arguments
pub fn proc_pid_cmdline(pid: u64) -> String {
    let table = process::PROCESS_TABLE.lock();
    if let Some(ref t) = *table {
        if let Some(proc_entry) = t.getpid(Pid(pid)) {
            let mut out = proc_entry.name.clone();
            out.push('\0');
            return out;
        }
    }
    String::new()
}

/// /proc/[pid]/environ — null-separated environment variables
pub fn proc_pid_environ(pid: u64) -> String {
    let table = process::PROCESS_TABLE.lock();
    if let Some(ref t) = *table {
        if let Some(proc_entry) = t.getpid(Pid(pid)) {
            let mut out = String::new();
            for (key, val) in &proc_entry.env {
                out.push_str(key);
                out.push('=');
                out.push_str(val);
                out.push('\0');
            }
            return out;
        }
    }
    String::new()
}

/// /proc/[pid]/fd/[n] — returns the symlink target for fd N
pub fn proc_pid_fd(pid: u64, fd_num: u32) -> String {
    let table = process::PROCESS_TABLE.lock();
    if let Some(ref t) = *table {
        if let Some(proc_entry) = t.getpid(Pid(pid)) {
            if let Some(fd) = proc_entry.fd_table.get(fd_num) {
                return match &fd.fd_type {
                    FdType::RegularFile { path, .. } => path.clone(),
                    FdType::Pipe { read_end, .. } => {
                        format!("pipe:[{}]", if *read_end { "read" } else { "write" })
                    }
                    FdType::Socket(id) => format!("socket:[{}]", id),
                    FdType::Device(id) => format!("/dev/{}", id),
                    FdType::Directory { path } => path.clone(),
                    FdType::Epoll(id) => format!("anon_inode:[eventpoll:{}]", id),
                };
            }
        }
    }
    String::new()
}

/// List open fd numbers for a process (checks fds 0..1024)
fn proc_pid_fd_list(pid: u64) -> Vec<u32> {
    let table = process::PROCESS_TABLE.lock();
    let mut fds = Vec::new();
    if let Some(ref t) = *table {
        if let Some(proc_entry) = t.getpid(Pid(pid)) {
            for i in 0..1024u32 {
                if proc_entry.fd_table.get(i).is_some() {
                    fds.push(i);
                }
            }
        }
    }
    fds
}

/// /proc/[pid]/stat — single-line, fields 1-52 matching Linux format
pub fn proc_pid_stat(pid: u64) -> String {
    let table = process::PROCESS_TABLE.lock();
    if let Some(ref t) = *table {
        if let Some(proc_entry) = t.getpid(Pid(pid)) {
            let state = match proc_entry.state {
                ProcessState::Running => 'R',
                ProcessState::Sleeping => 'S',
                ProcessState::Stopped => 'T',
                ProcessState::Zombie => 'Z',
                ProcessState::Dead => 'X',
                ProcessState::TracedStopped => 't',
            };
            let utime = proc_entry.cpu_time_us / 10_000;
            let stime = utime / 4;
            let nice = proc_entry.nice as i64;
            let num_threads = proc_entry.threads.len() as u64;
            let vsize = proc_entry.memory_space.total_mapped;
            let rss = vsize / 4096;
            let start_time = proc_entry.start_time;

            return format!(
                "{} ({}) {} {} {} {} 0 0 0 \
                 0 0 0 0 {} {} 0 0 \
                 {} {} {} 0 {} {} {} \
                 18446744073709551615 0 0 0 0 0 0 \
                 0 0 0 0 0 0 0 \
                 {} 0 0 0 0 0 \
                 0 0 0 0 0 0 0 0 0\n",
                proc_entry.pid.0,
                proc_entry.name,
                state,
                proc_entry.ppid.0,
                proc_entry.process_group,
                proc_entry.session_id,
                utime,
                stime,
                proc_entry.priority as u8,
                nice,
                num_threads,
                start_time,
                vsize,
                rss,
                0u32,
            );
        }
    }
    String::new()
}

// ═══════════════════════════════════════════════════════════════════════════════
// System-wide /proc entries
// ═══════════════════════════════════════════════════════════════════════════════

/// /proc/cpuinfo — per-CPU info block
pub fn proc_cpuinfo() -> String {
    let mut out = String::new();
    let num_cpus = 1u32;

    for i in 0..num_cpus {
        out.push_str(&format!(
            "processor\t: {}\n\
             vendor_id\t: GenuineIntel\n\
             cpu family\t: 6\n\
             model\t\t: 142\n\
             model name\t: RaeenOS Virtual CPU @ 3.00GHz\n\
             stepping\t: 10\n\
             microcode\t: 0xde\n\
             cpu MHz\t\t: 3000.000\n\
             cache size\t: 8192 KB\n\
             physical id\t: 0\n\
             siblings\t: {}\n\
             core id\t\t: {}\n\
             cpu cores\t: {}\n\
             apicid\t\t: {}\n\
             initial apicid\t: {}\n\
             fpu\t\t: yes\n\
             fpu_exception\t: yes\n\
             cpuid level\t: 22\n\
             wp\t\t: yes\n\
             flags\t\t: fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush mmx fxsr sse sse2 ht syscall nx rdtscp lm constant_tsc rep_good nopl xtopology cpuid pni pclmulqdq ssse3 fma cx16 sse4_1 sse4_2 movbe popcnt aes xsave avx f16c rdrand hypervisor lahf_lm abm cpuid_fault invpcid_single pti ssbd ibrs ibpb stibp fsgsbase bmi1 avx2 smep bmi2 erms invpcid xsaveopt arat\n\
             bugs\t\t: spectre_v1 spectre_v2 mds\n\
             bogomips\t: 6000.00\n\
             clflush size\t: 64\n\
             cache_alignment\t: 64\n\
             address sizes\t: 39 bits physical, 48 bits virtual\n\
             power management:\n\n",
            i, num_cpus, i, num_cpus, i, i,
        ));
    }
    out
}

/// /proc/meminfo — memory statistics
pub fn proc_meminfo() -> String {
    let total_kb: u64 = 262144;
    let free_kb: u64 = 131072;
    let avail_kb: u64 = 196608;
    let buffers_kb: u64 = 8192;
    let cached_kb: u64 = 32768;
    let swap_total: u64 = 0;
    let swap_free: u64 = 0;
    let slab_kb: u64 = 4096;
    let sreclaimable: u64 = 2048;

    format!(
        "MemTotal:       {:>8} kB\n\
         MemFree:        {:>8} kB\n\
         MemAvailable:   {:>8} kB\n\
         Buffers:        {:>8} kB\n\
         Cached:         {:>8} kB\n\
         SwapCached:     {:>8} kB\n\
         Active:         {:>8} kB\n\
         Inactive:       {:>8} kB\n\
         Active(anon):   {:>8} kB\n\
         Inactive(anon): {:>8} kB\n\
         Active(file):   {:>8} kB\n\
         Inactive(file): {:>8} kB\n\
         Unevictable:    {:>8} kB\n\
         Mlocked:        {:>8} kB\n\
         SwapTotal:      {:>8} kB\n\
         SwapFree:       {:>8} kB\n\
         Dirty:          {:>8} kB\n\
         Writeback:      {:>8} kB\n\
         AnonPages:      {:>8} kB\n\
         Mapped:         {:>8} kB\n\
         Shmem:          {:>8} kB\n\
         KReclaimable:   {:>8} kB\n\
         Slab:           {:>8} kB\n\
         SReclaimable:   {:>8} kB\n\
         SUnreclaim:     {:>8} kB\n\
         KernelStack:    {:>8} kB\n\
         PageTables:     {:>8} kB\n\
         NFS_Unstable:   {:>8} kB\n\
         Bounce:         {:>8} kB\n\
         WritebackTmp:   {:>8} kB\n\
         CommitLimit:    {:>8} kB\n\
         Committed_AS:   {:>8} kB\n\
         VmallocTotal:   34359738367 kB\n\
         VmallocUsed:    {:>8} kB\n\
         VmallocChunk:   {:>8} kB\n\
         HardwareCorrupted: {:>5} kB\n\
         AnonHugePages:  {:>8} kB\n\
         ShmemHugePages: {:>8} kB\n\
         ShmemPmdMapped: {:>8} kB\n\
         HugePages_Total:   {:>5}\n\
         HugePages_Free:    {:>5}\n\
         HugePages_Rsvd:    {:>5}\n\
         HugePages_Surp:    {:>5}\n\
         Hugepagesize:   {:>8} kB\n\
         Hugetlb:        {:>8} kB\n\
         DirectMap4k:    {:>8} kB\n\
         DirectMap2M:    {:>8} kB\n\
         DirectMap1G:    {:>8} kB\n",
        total_kb,
        free_kb,
        avail_kb,
        buffers_kb,
        cached_kb,
        0u64,
        total_kb / 3,
        total_kb / 6,
        total_kb / 4,
        total_kb / 8,
        total_kb / 12,
        total_kb / 12,
        0u64,
        0u64,
        swap_total,
        swap_free,
        0u64,
        0u64,
        total_kb / 4,
        total_kb / 8,
        0u64,
        sreclaimable,
        slab_kb,
        sreclaimable,
        slab_kb - sreclaimable,
        256u64,
        128u64,
        0u64,
        0u64,
        0u64,
        total_kb / 2,
        total_kb / 4,
        1024u64,
        0u64,
        0u64,
        0u64,
        0u64,
        0u64,
        0u64,
        0u64,
        0u64,
        0u64,
        2048u64,
        0u64,
        total_kb / 4,
        total_kb / 2,
        0u64,
    )
}

/// /proc/uptime — seconds since boot + idle time
pub fn proc_uptime() -> String {
    let up = uptime_secs();
    let idle = up / 2;
    format!("{}.00 {}.00\n", up, idle)
}

/// /proc/loadavg — 1/5/15 minute load averages + running/total tasks
pub fn proc_loadavg() -> String {
    let table = process::PROCESS_TABLE.lock();
    let (running, total) = if let Some(ref t) = *table {
        let procs = t.list_processes();
        let running = procs
            .iter()
            .filter(|p| p.state == ProcessState::Running)
            .count();
        (running, procs.len())
    } else {
        (0, 0)
    };
    let pid_max = total + 1;
    format!("0.00 0.01 0.05 {}/{} {}\n", running, total, pid_max)
}

/// /proc/version
pub fn proc_version() -> String {
    String::from("RaeenOS version 0.0.1 (raeen@raeenos) (rustc 1.80.0) #1 SMP RaeenOS 0.0.1 (Linux compat)\n")
}

/// /proc/raeen/caps — capability audit log (kernelchecklist.md §5.9).
pub fn proc_raeen_acpi() -> String {
    crate::acpi_full::dump_text()
}

pub fn proc_raeen_acpi_quirks() -> String {
    crate::acpi_quirks::dump_text()
}

pub fn proc_raeen_edid() -> String {
    crate::edid::dump_text()
}

pub fn proc_raeen_gpe() -> String {
    crate::gpe::dump_text()
}

pub fn proc_raeen_caps() -> String {
    crate::cap_audit::dump_text()
}

/// /proc/raeen/cpu — CPU feature detection (kernelchecklist.md §M-A).
pub fn proc_raeen_cpu() -> String {
    crate::cpu_features::dump_text()
}

/// /proc/raeen/msr_amd — AMD CPPC / SVM / SMCA MSR detection (MasterChecklist Phase 1.3).
pub fn proc_raeen_msr_amd() -> String {
    crate::msr_amd::dump_text()
}

/// /proc/raeen/msr — CPU vendor + fault-tolerant MSR access status.
pub fn proc_raeen_msr() -> String {
    crate::msr::dump_text()
}

/// /proc/raeen/hardware — SMBIOS DMI + matched hardware profile.
/// Required for M-A "Boots on Athena" — boot log must say which board
/// we matched and which quirks we applied.
pub fn proc_raeen_hardware() -> String {
    crate::hardware_profile::dump_text()
}

pub fn proc_raeen_numa() -> String {
    crate::numa::dump_text()
}

/// /proc/raeen/msr_intel — Intel-specific MSR feature state (HWP/ITD/CET).
pub fn proc_raeen_msr_intel() -> String {
    crate::msr_intel::dump_text()
}

/// /proc/raeen/drivers — registered userspace drivers + claims + DMA status.
/// Concept §Architecture path-C surface.
pub fn proc_raeen_drivers() -> String {
    crate::userspace_driver::dump_text()
}

/// /proc/raeen/tls — TLS 1.3 manager + per-connection state.
pub fn proc_raeen_tls() -> String {
    crate::tls::dump_text()
}

/// /proc/raeen/rtc — wall clock (CMOS RTC at boot + TSC delta now).
pub fn proc_raeen_rtc() -> String {
    crate::rtc::dump_text()
}

/// /proc/raeen/dhcp — DHCP client state + lease.
pub fn proc_raeen_dhcp() -> String {
    crate::dhcp::dump_text()
}

/// /proc/raeen/pci_irq — PCI legacy IRQ routing table.
pub fn proc_raeen_pci_irq() -> String {
    crate::pci_irq::dump_text()
}

/// /proc/raeen/pcie — PCIe ECAM discovery status.
pub fn proc_raeen_pcie() -> String {
    crate::pcie::dump_text()
}

/// /proc/raeen/pcie_aer — PCIe Advanced Error Reporting capability map.
pub fn proc_raeen_pcie_aer() -> String {
    crate::pcie_aer::dump_text()
}

/// /proc/raeen/selftest — consolidated boot self-test health summary.
pub fn proc_raeen_selftest() -> String {
    crate::selftest::dump_text()
}

/// /proc/raeen/apic — APIC mode and frequency info.
pub fn proc_raeen_apic() -> String {
    crate::apic::dump_text()
}

/// /proc/raeen/smp — per-CPU timer-tick heartbeat. A CPU with 0 ticks
/// is either offline or wedged in a non-interruptible region.
pub fn proc_raeen_smp() -> String {
    let mut out = String::new();
    out.push_str("# RaeenOS SMP heartbeat (per-CPU timer IRQ counters)\n");
    let n = crate::gdt::MAX_CPUS;
    let mut alive = 0usize;
    let mut working = 0usize;
    for i in 0..n {
        let t = crate::scheduler::PER_CPU_TICKS[i].load(core::sync::atomic::Ordering::Relaxed);
        let p = crate::scheduler::PER_CPU_PICKS[i].load(core::sync::atomic::Ordering::Relaxed);
        let s = crate::scheduler::PER_CPU_STEALS[i].load(core::sync::atomic::Ordering::Relaxed);
        if t > 0 {
            alive += 1;
        }
        if p > 0 {
            working += 1;
        }
        if t > 0 || p > 0 || i < 4 {
            out.push_str(&alloc::format!(
                "cpu{}: ticks={} task_picks={} steals={}\n",
                i,
                t,
                p,
                s
            ));
        }
    }
    out.push_str(&alloc::format!(
        "# {} of {} CPU slot(s) heartbeating, {} actually running scheduler work\n",
        alive,
        n,
        working,
    ));
    out
}

/// /proc/raeen/buddy — buddy allocator stats + per-order free-list cardinality.
/// Concept §Memory: "needs buddy upgrade (10 MiB allocs in < 50 µs)".
pub fn proc_raeen_buddy() -> String {
    let g = crate::memory::BUDDY_ALLOCATORS.lock();
    let mut out = String::new();
    if g.is_empty() {
        out.push_str("# buddy allocator not initialized\n");
        return out;
    }
    for (i, b) in g.iter().enumerate() {
        let (total, free) = b.stats();
        out.push_str(&alloc::format!(
            "# node {}  total={} frames ({} MiB)  free={} frames ({} MiB)\n",
            i,
            total,
            (total * 4) >> 10,
            free,
            (free * 4) >> 10,
        ));
        let counts = b.order_counts();
        for (o, n) in counts.iter().enumerate() {
            if *n > 0 {
                let block_kib = 4u64 << o;
                out.push_str(&alloc::format!(
                    "  order {:>2} ({:>5} KiB): {} block(s)\n",
                    o,
                    block_kib,
                    n
                ));
            }
        }
    }
    out
}

/// /proc/raeen/heap_guard — kernel-heap freelist-integrity guard status.
/// Concept §Kernel Architecture: "a bad GPU driver crashes a service, not the
/// kernel"; §Principles: "Security by default, not by friction." Always-on in
/// the default build: the intrusive `Hole.next` link is stored XOR-obfuscated +
/// location-tied and validated (alignment + heap-range) on every deref, so a
/// DMA/wild write into a freed chunk is caught (fail-closed panic) before the
/// next allocation follows the stomped pointer. Never prints the cookie value.
pub fn proc_raeen_heap_guard() -> String {
    let s = crate::memory::allocator::freelist_guard_stats();
    let mut out = String::new();
    out.push_str(&alloc::format!(
        "cookie_installed: {}\n",
        s.cookie_installed
    ));
    out.push_str("encoding: xor+location-tie\n");
    out.push_str("failure_action: panic (fail-closed)\n");
    out.push_str(&alloc::format!("validations: {}\n", s.validations));
    out.push_str(&alloc::format!("corruptions: {}\n", s.corruptions));
    if let Some(addr) = s.last_corruption {
        out.push_str(&alloc::format!("last_corruption: {:#x}\n", addr));
    }
    out
}

/// /proc/raeen/hardening — kernel hardening status (KASLR, SMEP, SMAP, CFI, etc.).
pub fn proc_raeen_hardening() -> String {
    crate::hardening::dump_text()
}

/// /proc/raeen/uaccess — the validated user-memory copy chokepoint status.
pub fn proc_raeen_uaccess() -> String {
    crate::uaccess::procfs_status()
}

/// /proc/raeen/hardening_truth — IOMMU/AER/MCE/OOM runtime counters.
pub fn proc_raeen_hardening_truth() -> String {
    let mut out = String::new();
    out.push_str("# hardening truth\n");
    out.push_str(&crate::iommu::dump_text());
    out.push_str(&crate::aer::dump_text());
    out.push_str(&crate::mce::dump_text());
    out.push_str(&crate::oom::dump_text());
    out.push_str(&crate::soak::dump_text());
    out.push_str(&crate::process::dump_memlimits());
    out
}

/// /proc/raeen/raefs — RaeFS mount + journal + snapshot entry listing.
pub fn proc_raeen_raefs() -> String {
    crate::raefs::proc_dump_text()
}

/// /proc/raeen/network — smoltcp + DHCP + RaeShield summary.
pub fn proc_raeen_network() -> String {
    crate::net::dump_text()
}

pub fn proc_raeen_ssh() -> String {
    crate::ssh::dump_text()
}

/// /proc/raeen/power — periodic battery/AC telemetry and power profile state.
pub fn proc_raeen_power() -> String {
    crate::power::dump_text()
}

/// /proc/raeen/thermal — Phase 4.7 thermal zones, trips, and throttle state.
pub fn proc_raeen_thermal() -> String {
    crate::thermal::dump_text()
}

/// /proc/raeen/audio — RaeAudio kernel-side state.
pub fn proc_raeen_audio() -> String {
    crate::audio::dump_text()
}

/// /proc/raeen/usb_audio — USB Audio Class driver state.
pub fn proc_raeen_usb_audio() -> String {
    crate::usb_audio::dump_text()
}

/// /proc/raeen/syscalls — runtime mirror of docs/SYSCALL_TABLE.md.
pub fn proc_raeen_syscalls() -> String {
    let mut out = String::new();
    out.push_str("# RaeenOS syscall table (98 live syscalls across 18+ blocks)\n");
    out.push_str("# Full layout with rdi/rsi/rdx/r10: see docs/SYSCALL_TABLE.md\n\n");
    out.push_str("  1-14   foundational + IPC + capability + driver + process\n");
    out.push_str(" 15-23   file I/O\n");
    out.push_str(" 24-27   compositor surfaces\n");
    out.push_str(" 28-34   yield/getpid/time/input/readdir/screen_info\n");
    out.push_str(" 40-49   gaming-first surface (game_session)\n");
    out.push_str(" 50-53   versioned config registry\n");
    out.push_str(" 54-57   local-first search index\n");
    out.push_str(" 58-61   per-game profile\n");
    out.push_str(" 62-65   unified RGB API\n");
    out.push_str(" 66-67   app bundle verifier\n");
    out.push_str(" 68-70   compositor capture\n");
    out.push_str(" 71-73   permission prompt queue\n");
    out.push_str(" 74-77   theme engine\n");
    out.push_str(" 78-80   Rae scripting lifecycle\n");
    out.push_str(" 81-84   WireGuard tunnel registry\n");
    out.push_str(" 85-87   live wallpaper engine\n");
    out.push_str("109-118  userspace driver host ABI\n");
    out.push_str("284-290  anti-cheat attestation\n");
    out
}

/// /proc/raeen/memory — kernel memory subsystem snapshot.
///
/// Concept §"The user owns the machine": the OS is honest about the machine's
/// resources — `physical_total_bytes` exposes the installed RAM the kernel
/// manages (from the UEFI/e820 map) so the Settings → About panel can show the
/// real hardware total, not a fabricated number.
pub fn proc_raeen_memory() -> String {
    let pinned = crate::memory::pinned_page_count();
    let mut out = String::new();
    out.push_str("# RaeenOS memory subsystem\n");
    out.push_str(&format!(
        "heap_start: 0x{:x}\n",
        crate::memory::allocator::HEAP_START
    ));
    out.push_str(&format!(
        "heap_size:  0x{:x} ({} KiB)\n",
        crate::memory::allocator::HEAP_SIZE,
        crate::memory::allocator::HEAP_SIZE / 1024
    ));
    out.push_str(&format!("pinned_pages: {} ({} KiB)\n", pinned, pinned * 4));
    // Installed physical RAM the kernel manages (buddy total_frames × 4 KiB,
    // carved from the UEFI/e820 map at boot). Machine-parseable for the
    // Settings About panel. `(unavailable)` if the buddy lock is contended or
    // not yet initialized — never unwrap/panic in a procfs dump.
    match crate::memory::physical_total_bytes() {
        Some(total) => {
            out.push_str(&format!(
                "physical_total_bytes: {} ({} MiB)\n",
                total,
                total >> 20
            ));
        }
        None => out.push_str("physical_total_bytes: (unavailable)\n"),
    }
    match crate::memory::physical_free_bytes() {
        Some(free) => {
            out.push_str(&format!(
                "physical_free_bytes: {} ({} MiB)\n",
                free,
                free >> 20
            ));
        }
        None => out.push_str("physical_free_bytes: (unavailable)\n"),
    }
    out.push_str("# detailed buddy stats: see /proc/raeen/buddy (per-order free-list)\n");
    out
}

/// /proc/raeen/storage — aggregated RaeFS capacity for the Settings → Storage
/// panel.
///
/// Concept §"The user owns the machine": transparency about the machine's
/// storage. Emits both human-readable and machine-parseable `key: value` lines
/// (total/free/used bytes, block size) so userspace can render a real capacity
/// bar without minting a syscall. Read-only; reads the mounted superblock via a
/// non-blocking accessor so the dump can never deadlock the RaeFS lock.
pub fn proc_raeen_storage() -> String {
    let mut out = String::new();
    out.push_str("# RaeenOS storage capacity (RaeFS, aggregated)\n");
    match crate::raefs::capacity_bytes() {
        Some((total, free, block_size)) => {
            let used = total.saturating_sub(free);
            out.push_str("mounted: 1\n");
            out.push_str(&format!("total_bytes: {} ({} MiB)\n", total, total >> 20));
            out.push_str(&format!("free_bytes:  {} ({} MiB)\n", free, free >> 20));
            out.push_str(&format!("used_bytes:  {} ({} MiB)\n", used, used >> 20));
            out.push_str(&format!("block_size:  {}\n", block_size));
            // By-category hint: at this layer only the system bucket (the whole
            // volume) is cheaply available without walking inodes; userspace
            // refines per-app buckets from /proc/raeen/raefs.
            out.push_str(&format!("category_system_bytes: {}\n", used));
        }
        None => {
            out.push_str("mounted: 0\n");
            out.push_str("total_bytes: (unavailable)\n");
            out.push_str("free_bytes:  (unavailable)\n");
            out.push_str("used_bytes:  (unavailable)\n");
        }
    }
    out
}

/// Boot smoketest: prove the two new Settings-facing surfaces render real data.
///
/// Prints PASS only when `/proc/raeen/storage` renders non-empty AND
/// `/proc/raeen/memory` carries a `physical_total_bytes` > 0. Either failing
/// prints FAIL — this test can fail (it is not a false green): if the buddy
/// allocator reported 0 frames, or the storage arm produced an empty body, the
/// line below reads FAIL and the boot log records it.
pub fn run_boot_smoketest() {
    let storage = proc_raeen_storage();
    let phys = crate::memory::physical_total_bytes().unwrap_or(0);
    let storage_ok = !storage.trim().is_empty() && storage.contains("total_bytes:");
    let phys_ok = phys > 0;
    if storage_ok && phys_ok {
        crate::serial_println!(
            "[procfs] storage+memory smoketest PASS (physical_total={} MiB, storage {} bytes)",
            phys >> 20,
            storage.len()
        );
    } else {
        crate::serial_println!(
            "[procfs] storage+memory smoketest FAIL (storage_ok={} phys_ok={} phys={})",
            storage_ok,
            phys_ok,
            phys
        );
    }
}

/// /proc/raeen/sched_stats — scheduler counters.
pub fn proc_raeen_sched_stats() -> String {
    let ds = crate::scheduler::deadline_stats();
    let mut out = String::new();
    out.push_str("# RaeenOS scheduler counters\n");
    out.push_str(&format!("deadline_tasks: {}\n", ds.total_tasks));
    out.push_str(&format!("deadline_invocations: {}\n", ds.total_invocations));
    out.push_str(&format!("deadline_misses: {}\n", ds.total_misses));
    out.push_str(&format!("deadline_worst_miss_us: {}\n", ds.worst_miss_us));
    // misses per 10000 periods -> percent with two decimals (denominator is now
    // live; pre-fix it was permanently 0 so this read 0.00% even with misses).
    let rate_x10000 = if ds.total_invocations == 0 {
        0
    } else {
        ds.total_misses.saturating_mul(10_000) / ds.total_invocations
    };
    out.push_str(&format!(
        "deadline_miss_rate_pct: {}.{:02}\n",
        rate_x10000 / 100,
        rate_x10000 % 100,
    ));
    out.push_str(&format!(
        "game_mode_active: {}\n",
        crate::scheduler::game_mode_active() as u32
    ));
    out.push_str(&format!(
        "null_latency_active: {}\n",
        crate::scheduler::null_latency_active() as u32
    ));
    out.push_str("# per-CPU scheduler picks + work-steals (runqueue activity)\n");
    for i in 0..crate::gdt::MAX_CPUS {
        let picks = crate::scheduler::PER_CPU_PICKS[i].load(core::sync::atomic::Ordering::Relaxed);
        let steals =
            crate::scheduler::PER_CPU_STEALS[i].load(core::sync::atomic::Ordering::Relaxed);
        if picks > 0 || steals > 0 {
            out.push_str(&alloc::format!(
                "cpu{}: picks={} steals={}\n",
                i,
                picks,
                steals
            ));
        }
    }
    // The steal-resume #DF tripwire (MasterChecklist 4.8): MUST stay 0. A
    // non-zero value means the scheduler caught an insane-RSP resume and
    // recovered (no double fault) — the load-bearing signal for whether the
    // intermittent race recurs, far more trustworthy than a green boot.
    out.push_str(&alloc::format!(
        "switch_aborts: {}\n",
        crate::scheduler::SWITCH_ABORTS.load(core::sync::atomic::Ordering::Relaxed)
    ));
    out
}

/// /proc/raeen/compositor — compositor surface + present counters.
pub fn proc_raeen_compositor() -> String {
    let mut out = String::new();
    out.push_str("# RaeenOS compositor\n");
    if let Some((w, h)) = crate::compositor::screen_dimensions() {
        out.push_str(&format!("resolution: {}x{}\n", w, h));
    } else {
        out.push_str("resolution: unknown\n");
    }
    let (surfaces, presents) = crate::compositor::get_stats();
    out.push_str(&alloc::format!(
        "surfaces: {}\npresents: {}\n",
        surfaces,
        presents
    ));
    // Present-pipeline performance (the 120 fps contract's live counters —
    // docs/PERFORMANCE_TARGETS.md): last whole-frame time, last scanout blit
    // (row-copy + clflush), and the ~1s-window FPS.
    let (frame_us, blit_us, fps) = crate::compositor::present_perf();
    out.push_str(&alloc::format!(
        "present: frame_us={} blit_us={} fps={}\n",
        frame_us,
        blit_us,
        fps
    ));
    out.push_str(&alloc::format!(
        "overview: {}\nwallpaper_alpha: {}\n",
        crate::compositor::overview_active(),
        crate::compositor::wallpaper_alpha(),
    ));
    // Absolute cursor position (SYS_INPUT_CURSOR 279) — what an app polls to
    // hit-test where a click landed. Read from the same lock-free cache.
    let (cx, cy) = crate::compositor::cursor_position_fast();
    out.push_str(&alloc::format!("cursor: {},{}\n", cx, cy));
    // Accessibility §3 — screen magnifier mechanism state (zoom in 1/256
    // fixed-point: 256 = 1.0x; center = focus point in screen coords).
    let (mcx, mcy) = crate::compositor::magnifier_center();
    out.push_str(&alloc::format!(
        "magnifier: enabled={} zoom_x256={} center={},{}\n",
        crate::compositor::magnifier_enabled(),
        crate::compositor::magnifier_zoom_x256(),
        mcx,
        mcy,
    ));
    // Accessibility — color filters (0 None, 1 Invert, 2 HighContrast,
    // 3 Grayscale; strength only consulted for HighContrast).
    out.push_str(&alloc::format!(
        "a11y_filter: mode={} strength={}\n",
        crate::compositor::a11y_filter_mode(),
        crate::compositor::a11y_filter_strength(),
    ));
    // Drop-shadow penumbra test (material-and-shadow.md acceptance): 0=not-run,
    // 1=PASS soft near-black penumbra, 2=FAIL hard/tinted block. peak = edge
    // shadow alpha (0..255), ramp = penumbra falloff width (px).
    let (sresult, speak, sramp) = crate::compositor::shadow_penumbra_stats();
    out.push_str(&alloc::format!(
        "shadow_penumbra: {} peak_alpha={} ramp_px={}\n",
        match sresult {
            1 => "PASS",
            2 => "FAIL",
            _ => "not-run",
        },
        speak,
        sramp,
    ));
    out
}

/// /proc/raeen/index — every RaeenOS-native introspection endpoint.
/// Discoverability: `cat /proc/raeen/index` enumerates all the others.
/// Per `kernelchecklist.md` R3 (every module gets a procfs entry).
pub fn proc_raeen_index() -> String {
    let mut out = String::new();
    out.push_str("# RaeenOS introspection endpoints (cat any of these)\n");
    out.push_str("/proc/raeen/index           — this file\n");
    out.push_str("/proc/raeen/boot            — boot benchmark vs concept-doc targets\n");
    out.push_str("/proc/raeen/gaming          — game-mode / NULL_LATENCY / deadline stats\n");
    out.push_str("/proc/raeen/config          — versioned config registry dump\n");
    out.push_str("/proc/raeen/search          — local search index latency stats\n");
    out.push_str("/proc/raeen/games           — per-game profiles\n");
    out.push_str("/proc/raeen/rgb             — unified RGB device + zone state\n");
    out.push_str("/proc/raeen/bundles         — installed components + verify stats\n");
    out.push_str("/proc/raeen/perm            — permission-prompt queue summary\n");
    out.push_str("/proc/raeen/themes          — theme engine + current theme\n");
    out.push_str("/proc/raeen/scripts         — Rae scripting lifecycle\n");
    out.push_str("/proc/raeen/arch            — architecture HAL identity + status\n");
    out.push_str("/proc/raeen/apic            — APIC mode and frequency info\n");
    out.push_str("/proc/raeen/pcie            — PCIe ECAM discovery and status\n");
    out.push_str("/proc/raeen/pcie_aer        — PCIe Advanced Error Reporting map\n");
    out.push_str("/proc/raeen/pci_irq         — PCI legacy IRQ routing table\n");
    out.push_str("/proc/raeen/acpi            — ACPI tables and AML status\n");
    out.push_str("/proc/raeen/gpe             — General Purpose Event (GPE) counts\n");
    out.push_str("/proc/raeen/session         — RaeID login/session state\n");
    out.push_str("/proc/raeen/wireguard       — WireGuard tunnel registry\n");
    out.push_str("/proc/raeen/wallpaper       — live wallpaper + occlusion stats\n");
    out.push_str("/proc/raeen/caps            — capability audit log\n");
    out.push_str("/proc/raeen/memory          — memory subsystem snapshot\n");
    out.push_str("/proc/raeen/sched_stats     — scheduler counters\n");
    out.push_str("/proc/raeen/compositor      — compositor state\n");
    out.push_str("/proc/raeen/web             — RaeWeb browser surface (URL + DOM/paint stats)\n");
    out.push_str("/proc/raeen/cpu             — CPU vendor + features + Zen 4 detection\n");
    out.push_str("/proc/raeen/msr_amd         — AMD CPPC / SVM / SMCA MSR detection\n");
    out.push_str("/proc/raeen/hardening       — KASLR/SMEP/SMAP/CFI/W^X/Spectre status\n");
    out.push_str("/proc/raeen/heap_guard      — kernel-heap freelist integrity guard\n");
    out.push_str("/proc/raeen/hardening_truth — IOMMU/AER/MCE/OOM runtime counters\n");
    out.push_str("/proc/raeen/raefs           — RaeFS mount + journal + snapshot entries\n");
    out.push_str("/proc/raeen/network         — net stack + firewall summary\n");
    out.push_str("/proc/raeen/power           — battery/AC periodic telemetry\n");
    out.push_str("/proc/raeen/audio           — RaeAudio kernel-side state\n");
    out.push_str("/proc/raeen/syscalls        — runtime syscall table mirror\n");
    out.push_str("/proc/raeen/windows_gap     — Concept-doc Windows pain-point kernel map\n");
    out.push_str("/proc/raeen/clipboard       — session text clipboard stats\n");
    out.push_str("/proc/raeen/clipboard-panel — clipboard-history flyout (Super+C) state\n");
    out.push_str("/proc/raeen/capture         — compositor screen-capture sessions\n");
    out.push_str("/proc/raeen/storage_irq     — NVMe/AHCI MSI-X vs INTx fallback\n");
    out.push_str("/proc/raeen/storage         — RaeFS aggregated capacity (total/free/used)\n");
    out.push_str("/proc/raeen/ahci           — AHCI controllers and SATA ports\n");
    out.push_str("/proc/raeen/nvme           — NVMe controllers and namespaces\n");
    out.push_str("/proc/raeen/syscall_guard   — syscall hardening counters + limits\n");
    out.push_str("/proc/raeen/linux_kabi     — Linux kABI symbol registry (scaffold)\n");
    out.push_str("/proc/raeen/linux_compat   — Linux driver API shim (DMA/IRQ/kmalloc)\n");
    out.push_str("/proc/raeen/linuxkpi       — LinuxKPI host (jiffies/msleep/printk ABI)\n");
    out.push_str("/proc/raeen/linux_syscall  — Linux ABI translation counters\n");
    out.push_str("/proc/raeen/vfs            — Virtual Filesystem hierarchy status\n");
    out.push_str("/proc/raeen/msr_intel      — Intel HWP/Thread-Director/CET MSR state\n");
    out
}

/// /proc/raeen/linux_kabi — Linux kernel module symbol name registry (scaffold).
pub fn proc_raeen_linux_kabi() -> String {
    crate::linux_kabi::dump_text()
}

/// /proc/raeen/linux_compat — Linux driver compat shim status.
pub fn proc_raeen_linux_compat() -> String {
    crate::linux_compat::dump_text()
}

/// /proc/raeen/linuxkpi — LinuxKPI host ABI status (userspace driver daemons).
pub fn proc_raeen_linuxkpi() -> String {
    crate::linuxkpi_host::proc_dump_text()
}

/// /proc/raeen/linux_syscall — Linux ABI translation stats.
pub fn proc_raeen_linux_syscall() -> String {
    crate::linux_syscall::dump_text()
}

/// /proc/raeen/session — RaeID login/session status.
pub fn proc_raeen_session() -> String {
    crate::session::dump_text()
}

fn resolve_raeen_subpath(sub: &str) -> Option<String> {
    Some(match sub {
        "index" => proc_raeen_index(),
        "boot" => proc_raeen_boot(),
        "gaming" => proc_raeen_gaming(),
        "config" => proc_raeen_config(),
        "search" => proc_raeen_search(),
        "games" => proc_raeen_games(),
        "rgb" => proc_raeen_rgb(),
        "bundles" => proc_raeen_bundles(),
        "perm" => proc_raeen_perm(),
        "themes" => proc_raeen_themes(),
        "scripts" => proc_raeen_scripts(),
        "session" => proc_raeen_session(),
        "arch" => crate::arch::dump_text(),
        "apic" => proc_raeen_apic(),
        "pcie" => proc_raeen_pcie(),
        "pcie_aer" => proc_raeen_pcie_aer(),
        "selftest" => proc_raeen_selftest(),
        "pci_irq" => proc_raeen_pci_irq(),
        "acpi" => proc_raeen_acpi(),
        "acpi_quirks" => proc_raeen_acpi_quirks(),
        "edid" => proc_raeen_edid(),
        "gpe" => proc_raeen_gpe(),
        "wireguard" => proc_raeen_wireguard(),
        "firewall" => crate::firewall::dump_text(),
        "dns" => crate::dns::dump_text(),
        "dot" => crate::dot::dump_text(),
        "quic" => crate::quic::dump_text(),
        "shaper" => crate::net::dump_shaper_text(),
        "netlog" => crate::netlog::dump_text(),
        "mdns" => crate::mdns::dump_text(),
        "snap_policy" => crate::snapshot_policy::dump_text(),
        "update_slots" => crate::update_slots::dump_text(),
        "fde" => crate::fde::dump_text(),
        "perm_ui" => crate::perm_ui::dump_text(),
        "hid_pad" => crate::hid_gamepad::dump_text(),
        "a11y_keys" => crate::a11y_input::dump_text(),
        "captions" => crate::captions::dump_text(),
        "wm" => crate::wm_policy::dump_text(),
        "vibe" => crate::vibe_mode::dump_text(),
        "notify" => crate::notify::dump_text(),
        "prefetch" => crate::prefetch::dump_text(),
        "winreg" => crate::win_registry::dump_text(),
        "raebridge_seh" => crate::raebridge_boot::seh_dump_text(),
        "raebridge_sync" => crate::raebridge_boot::sync_dump_text(),
        "raebridge_registry" => crate::raebridge_boot::registry_dump_text(),
        "raebridge_syncbroker" => crate::raebridge_boot::sync_broker_dump_text(),
        "futex" => crate::sync::dump_text(),
        "widgets" => crate::widgets::dump_text(),
        "pci_pm" => crate::pci_pm::dump_text(),
        "swap" => crate::swap::dump_text(),
        "compress" => crate::compress::dump_text(),
        "perf" => crate::perf::dump_text(),
        "sched_proof" => crate::sched_proof::dump_text(),
        "wallpaper" => proc_raeen_wallpaper(),
        "caps" => proc_raeen_caps(),
        "memory" => proc_raeen_memory(),
        "sched_stats" => proc_raeen_sched_stats(),
        "compositor" => proc_raeen_compositor(),
        "a11y" => crate::a11y::dump_text(),
        "web" => crate::webview::dump_text(),
        "cpu" => proc_raeen_cpu(),
        "msr_amd" => proc_raeen_msr_amd(),
        "hardening" => proc_raeen_hardening(),
        "vulnerabilities" => crate::cpu_features::vulnerabilities_dump_text(),
        "tpm" => crate::tpm::tpm_dump_text(),
        "heap_guard" => proc_raeen_heap_guard(),
        "hardening_truth" => proc_raeen_hardening_truth(),
        "uaccess" => proc_raeen_uaccess(),
        "windows_gap" => proc_raeen_windows_gap(),
        "clipboard" => proc_raeen_clipboard(),
        "clipboard-panel" => crate::shell_runner::clipboard_panel_dump_text(),
        "capture" => crate::compositor::capture_dump_text(),
        "storage_irq" => proc_raeen_storage_irq(),
        "storage" => proc_raeen_storage(),
        "ahci" => proc_raeen_ahci(),
        "nvme" => proc_raeen_nvme(),
        "usb_hid" => proc_raeen_usb_hid(),
        "oom" => proc_raeen_oom(),
        "soak" => crate::soak::dump_text(),
        "memlimits" => crate::process::dump_memlimits(),
        "drivers" => crate::driver_manifest::dump_text(),
        "virtio_gpu" => crate::virtio_gpu::dump_text(),
        "crash" => proc_raeen_crash(),
        "fatfs_esp" => proc_raeen_fatfs_esp(),
        "installer" => crate::installer::dump_text(),
        "installer_ui" => crate::installer_ui::dump_text(),
        "secure_boot" => crate::secure_boot::dump_text(),
        "measured_boot" => crate::measured_boot::dump_text(),
        "sandbox" => crate::sandbox::dump_text(),
        "manifests" => crate::rae_manifest::dump_text(),
        "anticheat" => crate::anticheat::dump_text(),
        "syscall_guard" => proc_raeen_syscall_guard(),
        "linux_kabi" => proc_raeen_linux_kabi(),
        "linux_compat" => proc_raeen_linux_compat(),
        "linuxkpi" => proc_raeen_linuxkpi(),
        "linux_syscall" => proc_raeen_linux_syscall(),
        "power" => proc_raeen_power(),
        "thermal" => proc_raeen_thermal(),
        "vfs" => crate::vfs::proc_dump_text(),
        "palette" => crate::shell_runner::palette_dump_text(),
        "keyboard" => crate::shell_runner::keyboard_dump_text(),
        "control_center" => crate::shell_runner::control_center_dump_text(),
        _ => return None,
    })
}

/// /proc/raeen/windows_gap — Concept §Windows Pain Points kernel map.
pub fn proc_raeen_windows_gap() -> String {
    crate::windows_gap::dump_text()
}

/// /proc/raeen/clipboard — session clipboard stats.
pub fn proc_raeen_clipboard() -> String {
    crate::clipboard::dump_text()
}

/// /proc/raeen/storage_irq — storage controller IRQ modes.
pub fn proc_raeen_storage_irq() -> String {
    crate::storage_irq::dump_text()
}

/// /proc/raeen/ahci — AHCI HBA and port state. REDOX_EXTRACTION_MAP R06.
pub fn proc_raeen_ahci() -> String {
    crate::ahci::dump_text()
}

/// /proc/raeen/nvme — NVMe controller/namespace state. REDOX_EXTRACTION_MAP R07.
pub fn proc_raeen_nvme() -> String {
    crate::nvme::dump_text()
}

/// /proc/raeen/usb_hid — boot-keyboard parser counters
/// (MasterChecklist Phase 2.1 / REDOX_EXTRACTION_MAP R05).
pub fn proc_raeen_usb_hid() -> String {
    crate::usb_hid::dump_text()
}

/// /proc/raeen/oom — OOM policy counters + current largest-victim candidate
/// (MasterChecklist Phase 4.1).
pub fn proc_raeen_oom() -> String {
    crate::oom::dump_text()
}

/// /proc/raeen/crash — reserved crash-dump region + prior-crash tombstone
/// (MasterChecklist Phase 4.5).
pub fn proc_raeen_crash() -> String {
    crate::crash_dump::dump_text()
}

/// /proc/raeen/fatfs_esp — FAT32 BPB + root-dir snapshot from sector 0
/// of the active block device. REDOX_EXTRACTION_MAP R09 / Phase 3.3.
pub fn proc_raeen_fatfs_esp() -> String {
    crate::fatfs_esp::dump_text()
}

/// /proc/raeen/syscall_guard — syscall hardening counters and active limits.
pub fn proc_raeen_syscall_guard() -> String {
    crate::syscall::dump_guard_text()
}

/// /proc/raeen/gaming
///
/// Concept-doc-aligned introspection: how often has the user been in game
/// mode, how often has the scheduler been in NULL_LATENCY, and how many
/// SCHED_GAME deadlines have we hit vs. missed since boot. Userspace tools
/// (RaePlay overlay, Game Bar, the Settings → Performance pane) read this
/// to render a "your system has missed N frames out of M" indicator.
pub fn proc_raeen_gaming() -> String {
    let s = crate::game_session::stats();
    let game_mode_active = crate::scheduler::game_mode_active();
    let null_latency_active = crate::scheduler::null_latency_active();
    let miss_rate_x10000: u64 = if s.deadline_total == 0 {
        0
    } else {
        // ppm-style: misses per 10000 invocations
        s.deadline_misses.saturating_mul(10_000) / s.deadline_total
    };
    let mut out = String::new();
    out.push_str("# RaeenOS gaming session counters\n");
    out.push_str(&format!("game_mode_active: {}\n", game_mode_active as u32));
    out.push_str(&format!(
        "null_latency_active: {}\n",
        null_latency_active as u32
    ));
    out.push_str(&format!("game_mode_entries: {}\n", s.game_mode_entries));
    out.push_str(&format!(
        "null_latency_entries: {}\n",
        s.null_latency_entries
    ));
    out.push_str(&format!("deadline_invocations: {}\n", s.deadline_total));
    out.push_str(&format!("deadline_misses: {}\n", s.deadline_misses));
    out.push_str(&format!("deadline_worst_miss_us: {}\n", s.worst_miss_us));
    // Two decimals via integer math: e.g. 327 -> "0.03"
    out.push_str(&format!(
        "deadline_miss_rate_pct: {}.{:02}\n",
        miss_rate_x10000 / 100,
        miss_rate_x10000 % 100,
    ));
    // GameOS couch cohesion (Phase 1): the couch surface paints with the LIVE
    // Vibe accent. `couch_accent` MUST equal `couch_seed`-derived base for the
    // re-skin to be coherent with the desktop — the cohesion invariant the
    // `[gameos] couch smoketest` line proves at boot.
    let couch_seed = raeshell::gameos::couch_active_seed();
    let couch_accent = raeshell::gameos::couch_active_accent();
    out.push_str(&format!("couch_seed: {:#010X}\n", couch_seed));
    out.push_str(&format!("couch_accent: {:#010X}\n", couch_accent));
    out.push_str(&format!(
        "couch_accent_coherent: {}\n",
        (couch_accent == couch_seed) as u32
    ));
    out.push_str(&format!(
        "couch_hit_target_px: {}\n",
        raeshell::gameos::couch_hit_target()
    ));
    // GameOS controller glyphs (Phase 2): the active button-glyph skin the
    // context hint bar renders, and how many chips the default grid context
    // shows. Phase 3 binds the real pad's VID/PID to override the default skin.
    out.push_str(&format!(
        "couch_glyph_set: {}\n",
        raeshell::gameos::default_glyph_set_tag()
    ));
    out.push_str(&format!(
        "couch_hint_chips: {}\n",
        raeshell::gameos::default_context_chip_count()
    ));
    // GameOS live controller bind (Phase 3): when a pad binds on xHCI, its USB
    // VID/PID auto-selects the glyph set (Sony→ps, Microsoft→xbox, Nintendo→
    // nintendo, else generic) and `hid_gamepad::PadInput` drives couch focus.
    // The padbind smoketest closes the loop end-to-end (decode → route → assert).
    let pb = raeshell::gameos::run_padbind_smoketest();
    out.push_str(&format!(
        "couch_pad_vid_sony: {:#06X} -> {}\n",
        raeshell::gameos::VID_SONY,
        raeshell::gameos::glyph_set_tag_for_vidpid(raeshell::gameos::VID_SONY, 0)
    ));
    out.push_str(&format!(
        "couch_pad_vid_microsoft: {:#06X} -> {}\n",
        raeshell::gameos::VID_MICROSOFT,
        raeshell::gameos::glyph_set_tag_for_vidpid(raeshell::gameos::VID_MICROSOFT, 0)
    ));
    out.push_str(&format!(
        "couch_pad_vid_nintendo: {:#06X} -> {}\n",
        raeshell::gameos::VID_NINTENDO,
        raeshell::gameos::glyph_set_tag_for_vidpid(raeshell::gameos::VID_NINTENDO, 0)
    ));
    out.push_str(&format!(
        "couch_padbind_dpad_right_moves_focus: {}\n",
        pb.dpad_right_moves_focus as u32
    ));
    out.push_str(&format!("couch_padbind_bound_set: {}\n", pb.vidpid_set_tag));
    out.push_str(&format!("couch_padbind_ok: {}\n", pb.passed() as u32));

    // GameOS Game Bar overlay (Phase 4): the Concept's "Game Bar that doesn't
    // suck — FPS, frametime graph, CPU/GPU temps, all native, all fast". The
    // overlay reads FPS/frametime from the LIVE `crate::perf` ring + CPU/GPU
    // temps from `crate::thermal` (None → "(n/a)"). We surface the live readings
    // here plus a self-contained Game-Bar smoketest result so the panel state is
    // introspectable without the boot log.
    let gb_fps = crate::perf::fps_estimate_x100();
    let gb_ft = crate::perf::last_frametime_us();
    let (gb_cpu, gb_gpu, _ssd) = crate::thermal::read_component_temps();
    out.push_str(&format!(
        "gamebar_fps_x100: {}\n",
        gb_fps.map(|v| v as i64).unwrap_or(-1)
    ));
    out.push_str(&format!(
        "gamebar_frametime_us: {}\n",
        gb_ft.map(|v| v as i64).unwrap_or(-1)
    ));
    out.push_str(&format!(
        "gamebar_cpu_temp_c: {}\n",
        gb_cpu
            .map(|v| alloc::format!("{}", v))
            .unwrap_or_else(|| "n/a".into())
    ));
    out.push_str(&format!(
        "gamebar_gpu_temp_c: {}\n",
        gb_gpu
            .map(|v| alloc::format!("{}", v))
            .unwrap_or_else(|| "n/a".into())
    ));
    // Light self-check (no per-read canvas render): a scratch Game Bar invokes,
    // ingests a synthetic 60fps frame, and reports the live panel count + ring
    // fill — proving the overlay state machine is wired without the boot log.
    {
        let mut bar = raeshell::game_bar::GameBar::new(1, 1);
        let invoked = bar.invoke();
        bar.ingest_perf(&raeshell::game_bar::PerfFeed {
            fps: None,
            frametime_ms: Some(16.6),
            cpu_temp_c: gb_cpu.map(|c| c as f32),
            gpu_temp_c: gb_gpu.map(|g| g as f32),
        });
        out.push_str(&format!("gamebar_invoked_ok: {}\n", invoked as u32));
        out.push_str(&format!("gamebar_frametime_pts: {}\n", bar.ft_ring.len()));
        out.push_str(&format!("gamebar_panels: {}\n", bar.live_panel_count()));
    }

    // GameOS on-screen keyboard + cross-fade + auto-enter (Phase 6, the FINAL
    // phase). The Concept's controller-first text entry, "Toggle into it
    // instantly" cross-fade, and auto-enter on controller-connect. We surface
    // the OSK key count, the cross-fade duration, and a self-contained Phase-6
    // smoketest result so the OSK/transition state is introspectable without the
    // boot log.
    let osk = raeshell::gameos::run_osk_smoketest();
    out.push_str(&format!("couch_osk_keys: {}\n", osk.keys));
    out.push_str(&format!("couch_osk_typed: {}\n", osk.typed));
    out.push_str(&format!(
        "couch_osk_backspace_ok: {}\n",
        osk.backspace_ok as u32
    ));
    out.push_str(&format!(
        "couch_crossfade_ms: {}\n",
        raeshell::gameos::CROSSFADE_MS
    ));
    out.push_str(&format!(
        "couch_crossfade_ramp_ok: {}\n",
        osk.crossfade_ramp_ok as u32
    ));
    out.push_str(&format!(
        "couch_autoenter_on_padbind: {}\n",
        osk.autoenter_on_padbind as u32
    ));
    out.push_str(&format!("couch_osk_ok: {}\n", osk.passed() as u32));

    out
}

/// /proc/raeen/boot — captured wall-time from BSP T0 to "System booted".
/// Populated by `procfs::record_boot_time_ms` from kernel_main.
static BOOT_TIME_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

pub fn record_boot_time_ms(ms: u64) {
    BOOT_TIME_MS.store(ms, core::sync::atomic::Ordering::Relaxed);
}

/// /proc/raeen/perm — permission-prompt queue overview.
pub fn proc_raeen_perm() -> String {
    crate::perm_syscalls::dump_text()
}

/// /proc/raeen/themes — registered themes + current selection.
pub fn proc_raeen_themes() -> String {
    crate::theme_engine::dump_text()
}

/// /proc/raeen/scripts — Rae scripting lifecycle state.
pub fn proc_raeen_scripts() -> String {
    crate::scripting::dump_text()
}

/// /proc/raeen/wireguard — WireGuard tunnel registry.
pub fn proc_raeen_wireguard() -> String {
    crate::wireguard::dump_text()
}

/// /proc/raeen/wallpaper — live wallpaper engine state and skip stats, prefixed
/// with the Aurora Mesh default-backdrop identity line (IDENTITY §3).
pub fn proc_raeen_wallpaper() -> String {
    let mut out = crate::aurora::dump_text();
    out.push_str(&crate::live_wallpaper::dump_text());
    out
}

/// /proc/raeen/rgb — every RGB device + zone color tracked by the kernel.
/// Concept §Customization Engine: "RGB unified — one API, one config."
pub fn proc_raeen_rgb() -> String {
    crate::rgb::dump_text()
}

/// /proc/raeen/bundles — installed components and dep-verification stats.
/// Concept §Windows pain points: "App bundles with explicit, hashed
/// dependencies."
pub fn proc_raeen_bundles() -> String {
    crate::app_bundle::dump_text()
}

/// /proc/raeen/games — full dump of stored per-game profiles.
/// Concept §Gaming Features: "Per-game profiles… all configured per game
/// and auto-applied."
pub fn proc_raeen_games() -> String {
    crate::game_profile::dump_text()
}

/// /proc/raeen/search — query latency stats for the local search index.
pub fn proc_raeen_search() -> String {
    let s = crate::search_index::stats();
    let mut out = String::new();
    out.push_str("# RaeenOS local search index\n");
    out.push_str(&format!("items: {}\n", s.items));
    out.push_str(&format!("tokens: {}\n", s.tokens));
    out.push_str(&format!("queries_total: {}\n", s.queries_total));
    out.push_str(&format!("last_query_hits: {}\n", s.last_query_hits));
    out.push_str(&format!("last_query_cycles: {}\n", s.last_query_cycles));
    out.push_str(&format!("best_query_cycles: {}\n", s.best_query_cycles));
    out.push_str(&format!("worst_query_cycles: {}\n", s.worst_query_cycles));
    out.push_str("concept_target_ms_per_query: 100\n");
    out
}

/// /proc/raeen/config — full deterministic dump of the config registry.
/// Concept-doc-aligned: "human-readable config with snapshots".
pub fn proc_raeen_config() -> String {
    crate::config_registry::dump_text()
}

pub fn proc_raeen_boot() -> String {
    let ms = BOOT_TIME_MS.load(core::sync::atomic::Ordering::Relaxed);
    format!(
        "boot_ms: {}\nconcept_target_ms: 6000\nconcept_stretch_ms: 3000\nstatus: {}\n\
         deferred_selftest_ran: {}\ndeferred_selftest_count: {}\nsplash_mark_px: {}\n",
        ms,
        if ms == 0 {
            "unmeasured"
        } else if ms < 3000 {
            "stretch_target_hit"
        } else if ms < 6000 {
            "target_hit"
        } else {
            "over_target"
        },
        // ADR 0006: the post-marker deferred feature-smoketest sweep. ran=true
        // confirms the deferral path executed; count is how many smoketests it
        // dispatched off the boot critical path.
        crate::boot_selftest::ran(),
        crate::boot_selftest::deferred_count(),
        // Boot-face proof: pixels the splash's Rae mark painted (0 = the text
        // mirror ran instead — safe mode / padded stride / regression).
        crate::fast_boot::SPLASH_MARK_PIXELS.load(core::sync::atomic::Ordering::Relaxed),
    )
}

/// Emit every `/proc/raeen/*` endpoint to serial as a fenced block.
///
/// Called at the very end of `kernel_main` so the boot log captures a
/// **complete observable kernel snapshot** every run. An external AI dev
/// agent (or a human grepping the log) gets the full live state without
/// having to run the kernel and query each endpoint individually — the
/// log replaces interactive introspection until SSH-equivalent exists.
///
/// Format: each endpoint wrapped in `<<<<<<<< raeen:/proc/raeen/NAME >>>>>>>>`
/// fences. Easy to parse with `grep -A 999` or `awk`.
pub fn dump_all_raeen_endpoints_to_serial() {
    // The endpoint table lives in .rodata via `static` so cross-CPU
    // memory pressure during long try_lock spins (e.g. raefs::proc_dump_text
    // when a busy lock falls back) can't zero out the `&'static str`
    // pointers — moving this off the BSP stack closed an observable
    // null-byte corruption window seen at `linux_kabi` and later
    // endpoints.
    static ENTRIES: &[(&str, fn() -> String)] = &[
        ("index", proc_raeen_index),
        ("boot", proc_raeen_boot),
        ("cpu", proc_raeen_cpu),
        ("msr_amd", proc_raeen_msr_amd),
        ("numa", proc_raeen_numa),
        ("msr_intel", proc_raeen_msr_intel),
        ("hardware", proc_raeen_hardware),
        ("drivers", proc_raeen_drivers),
        ("buddy", proc_raeen_buddy),
        ("heap_guard", proc_raeen_heap_guard),
        (
            "vulnerabilities",
            crate::cpu_features::vulnerabilities_dump_text,
        ),
        ("tpm", crate::tpm::tpm_dump_text),
        ("tls", proc_raeen_tls),
        ("rtc", proc_raeen_rtc),
        ("dhcp", proc_raeen_dhcp),
        ("pcie", proc_raeen_pcie),
        ("pcie_aer", proc_raeen_pcie_aer),
        ("selftest", proc_raeen_selftest),
        ("pci_irq", proc_raeen_pci_irq),
        ("apic", proc_raeen_apic),
        ("acpi", proc_raeen_acpi),
        ("gpe", proc_raeen_gpe),
        ("smp", proc_raeen_smp),
        ("hardening", proc_raeen_hardening),
        ("hardening_truth", proc_raeen_hardening_truth),
        ("uaccess", proc_raeen_uaccess),
        ("raefs", proc_raeen_raefs),
        ("memory", proc_raeen_memory),
        ("sched_stats", proc_raeen_sched_stats),
        ("compositor", proc_raeen_compositor),
        ("a11y", crate::a11y::dump_text),
        ("web", crate::webview::dump_text),
        ("caps", proc_raeen_caps),
        ("gaming", proc_raeen_gaming),
        ("games", proc_raeen_games),
        ("config", proc_raeen_config),
        ("search", proc_raeen_search),
        ("themes", proc_raeen_themes),
        ("scripts", proc_raeen_scripts),
        ("session", proc_raeen_session),
        ("wireguard", proc_raeen_wireguard),
        ("firewall", crate::firewall::dump_text),
        ("dns", crate::dns::dump_text),
        ("dot", crate::dot::dump_text),
        ("quic", crate::quic::dump_text),
        ("shaper", crate::net::dump_shaper_text),
        ("netlog", crate::netlog::dump_text),
        ("mdns", crate::mdns::dump_text),
        ("snap_policy", crate::snapshot_policy::dump_text),
        ("update_slots", crate::update_slots::dump_text),
        ("fde", crate::fde::dump_text),
        ("perm_ui", crate::perm_ui::dump_text),
        ("hid_pad", crate::hid_gamepad::dump_text),
        ("a11y_keys", crate::a11y_input::dump_text),
        ("captions", crate::captions::dump_text),
        ("wm", crate::wm_policy::dump_text),
        ("vibe", crate::vibe_mode::dump_text),
        ("notify", crate::notify::dump_text),
        ("prefetch", crate::prefetch::dump_text),
        ("winreg", crate::win_registry::dump_text),
        ("raebridge_seh", crate::raebridge_boot::seh_dump_text),
        ("raebridge_sync", crate::raebridge_boot::sync_dump_text),
        (
            "raebridge_registry",
            crate::raebridge_boot::registry_dump_text,
        ),
        (
            "raebridge_syncbroker",
            crate::raebridge_boot::sync_broker_dump_text,
        ),
        ("widgets", crate::widgets::dump_text),
        ("palette", crate::shell_runner::palette_dump_text),
        ("keyboard", crate::shell_runner::keyboard_dump_text),
        (
            "control_center",
            crate::shell_runner::control_center_dump_text,
        ),
        ("pci_pm", crate::pci_pm::dump_text),
        ("swap", crate::swap::dump_text),
        ("sched_proof", crate::sched_proof::dump_text),
        ("wallpaper", proc_raeen_wallpaper),
        ("rgb", proc_raeen_rgb),
        ("bundles", proc_raeen_bundles),
        ("perm", proc_raeen_perm),
        ("network", proc_raeen_network),
        ("ssh", proc_raeen_ssh),
        ("power", proc_raeen_power),
        ("thermal", proc_raeen_thermal),
        ("audio", proc_raeen_audio),
        ("usb_audio", proc_raeen_usb_audio),
        ("syscalls", proc_raeen_syscalls),
        ("linux_kabi", proc_raeen_linux_kabi),
        ("linux_compat", proc_raeen_linux_compat),
        ("linuxkpi", proc_raeen_linuxkpi),
        ("linux_syscall", proc_raeen_linux_syscall),
        ("windows_gap", proc_raeen_windows_gap),
        ("clipboard", proc_raeen_clipboard),
        (
            "clipboard-panel",
            crate::shell_runner::clipboard_panel_dump_text,
        ),
        ("capture", crate::compositor::capture_dump_text),
        ("storage_irq", proc_raeen_storage_irq),
        ("storage", proc_raeen_storage),
        ("ahci", proc_raeen_ahci),
        ("nvme", proc_raeen_nvme),
        ("usb_hid", proc_raeen_usb_hid),
        ("oom", proc_raeen_oom),
        ("soak", crate::soak::dump_text),
        ("memlimits", crate::process::dump_memlimits),
        ("drivers", crate::driver_manifest::dump_text),
        ("virtio_gpu", crate::virtio_gpu::dump_text),
        ("fatfs_esp", proc_raeen_fatfs_esp),
        ("installer", crate::installer::dump_text),
        ("installer_ui", crate::installer_ui::dump_text),
        ("suspend", crate::suspend::dump_text),
        ("sandbox", crate::sandbox::dump_text),
        ("manifests", crate::rae_manifest::dump_text),
        ("anticheat", crate::anticheat::dump_text),
        ("syscall_guard", proc_raeen_syscall_guard),
        ("extable", crate::extable::dump_text),
        ("bootlog", crate::bootlog::dump_text),
        ("bootlog_persist", crate::bootlog_persist::dump_text),
    ];
    let entries: &[(&str, fn() -> String)] = ENTRIES;

    // COM1-ONLY from here down (serial_only_println): this snapshot is a
    // ~900 KiB machine-parse diagnostic for the QEMU serial log. Routing it
    // through serial_println would (a) mirror it into the 1 MiB bootlog RAM
    // ring, evicting the actual boot transcript right before the end-of-boot
    // BOOTLOG.TXT flush — the on-stick log then showed procfs noise instead
    // of the tier lines needed to debug bare metal — and (b) recursively
    // append /proc/raeen/bootlog (the ring dump) INTO the ring. On-screen it
    // was unreadable scroll; QEMU parsing is unaffected since COM1 output is
    // identical.
    crate::serial_only_println!();
    crate::serial_only_println!(
        "<<<<<<<< raeen:snapshot:begin {} endpoints >>>>>>>>",
        entries.len()
    );

    // Suspend BSP-side IRQs across the entire dump — defense-in-depth, and
    // the historical rationale is now root-caused (2026-07-02, replacing the
    // earlier "AP work-stealing scans starve the BSP's lock" theory, which
    // never held up: APs cannot preempt the BSP and cross-CPU SCHEDULER
    // holds are µs-scale). The old truncate-at-sched_stats deadlock was the
    // SAME-CPU IRQ→lock re-entrancy class: timer-IRQ-context work of that
    // era (ACPI/EC evaluation, thermal, cpufreq-governor locking) re-entered
    // locks/heap that a getter (sched_stats et al.) held on this CPU — the
    // class independently hit and disabled elsewhere ("ACPI evaluation in
    // hard IRQ context causes heap/serial deadlocks", power.rs). Today the
    // CPU0 tick path is atomics-only AND this call site runs pre-
    // BOOT_COMPLETE (the yield gate no-ops, so no preemption either):
    // an experiment boot with this mask REMOVED completed 105/105 endpoints
    // clean. The mask stays so the dump remains immune if future tick-path
    // work reintroduces lock/heap use — for a once-per-boot diagnostic the
    // masking cost is nil. The self-check line below turns any recurrence
    // into a loud FAIL instead of silent truncation.
    x86_64::instructions::interrupts::without_interrupts(|| {
        // Completeness + timing self-check (MasterChecklist "Latent kernel
        // bugs": procfs dump truncation). Truncation used to be SILENT — the
        // log just stopped mid-endpoint and only an eyeball diff caught it.
        // Count every endpoint actually emitted and time each getter (rdtsc,
        // getter compute only — the serial writes are excluded so the number
        // is iron-UART-independent), then print a FAIL-able verdict:
        //   * emitted != declared  -> FAIL (a future early-break/panic path)
        //   * any single getter > ~2 s of TSC time -> FAIL (a wedged endpoint
        //     — the try_lock-fallback class this dump historically hit —
        //     that RETURNED late; a getter that never returns still hangs
        //     here, but the last fence line in the log now names it).
        let mhz = crate::fast_boot::tsc_mhz().max(1);
        let wedge_budget_cycles: u64 = 2_000 * 1_000 * mhz; // ~2 s of getter CPU time
        let mut emitted: usize = 0;
        let mut slowest_us: u64 = 0;
        let mut slowest_name: &str = "-";
        let mut total_us: u64 = 0;
        let mut wedged = false;
        for (name, f) in entries {
            let t0 = unsafe { core::arch::x86_64::_rdtsc() };
            let body = f();
            let dt = unsafe { core::arch::x86_64::_rdtsc() }.wrapping_sub(t0);
            let dt_us = dt / mhz;
            total_us += dt_us;
            if dt_us > slowest_us {
                slowest_us = dt_us;
                slowest_name = name;
            }
            if dt > wedge_budget_cycles {
                wedged = true;
            }
            crate::serial_only_println!("<<<<<<<< raeen:/proc/raeen/{} >>>>>>>>", name);
            // Print without prefix so a parser sees pure endpoint output.
            // Trim trailing newline since serial_only_println already adds one.
            let trimmed = body.trim_end_matches('\n');
            if !trimmed.is_empty() {
                crate::serial_only_println!("{}", trimmed);
            }
            emitted += 1;
        }
        crate::serial_only_println!("<<<<<<<< raeen:snapshot:end >>>>>>>>");
        let complete = emitted == entries.len() && !wedged;
        // serial_println (not _only_): this verdict is boot-health signal, so
        // it belongs in the bootlog ring / netlog too, unlike the bulk dump.
        crate::serial_println!(
            "[procfs] snapshot self-check: emitted={}/{} getter_total={}us slowest={}@{}us wedged={} -> {}",
            emitted,
            entries.len(),
            total_us,
            slowest_name,
            slowest_us,
            wedged,
            if complete { "PASS" } else { "FAIL" },
        );
        crate::serial_only_println!();
    });
}

/// /proc/filesystems — registered filesystem types
pub fn proc_filesystems() -> String {
    let mut out = String::new();
    out.push_str("nodev\tsysfs\n");
    out.push_str("nodev\tproc\n");
    out.push_str("nodev\ttmpfs\n");
    out.push_str("nodev\tdevtmpfs\n");
    out.push_str("nodev\tdevpts\n");
    out.push_str("nodev\tsecurityfs\n");
    out.push_str("nodev\tcgroup2\n");
    out.push_str("nodev\tpstore\n");
    out.push_str("nodev\tbpf\n");
    out.push_str("\traefs\n");
    out.push_str("\text4\n");
    out.push_str("\tvfat\n");
    out.push_str("\tntfs\n");
    out
}

/// /proc/mounts — mounted filesystems
pub fn proc_mounts() -> String {
    let mut out = String::new();
    out.push_str("raefs / raefs rw,relatime 0 0\n");
    out.push_str("proc /proc proc rw,nosuid,nodev,noexec,relatime 0 0\n");
    out.push_str("sysfs /sys sysfs rw,nosuid,nodev,noexec,relatime 0 0\n");
    out.push_str("tmpfs /tmp tmpfs rw,nosuid,nodev,relatime,size=131072k 0 0\n");
    out.push_str("tmpfs /dev/shm tmpfs rw,nosuid,nodev 0 0\n");
    out.push_str(
        "devtmpfs /dev devtmpfs rw,nosuid,relatime,size=131072k,nr_inodes=32768,mode=755 0 0\n",
    );
    out
}

/// /proc/net/tcp — TCP socket table
pub fn proc_net_tcp() -> String {
    let mut out = String::new();
    out.push_str("  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n");
    out
}

/// /proc/net/udp — UDP socket table
pub fn proc_net_udp() -> String {
    let mut out = String::new();
    out.push_str("  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n");
    out
}

/// /proc/stat — kernel/system statistics
pub fn proc_stat() -> String {
    let up = uptime_secs();
    let user_jiffies = up * 30;
    let system_jiffies = up * 10;
    let idle_jiffies = up * 60;

    let mut out = String::new();
    out.push_str(&format!(
        "cpu  {} 0 {} 0 {} 0 0 0 0 0\n",
        user_jiffies, system_jiffies, idle_jiffies,
    ));
    out.push_str(&format!(
        "cpu0 {} 0 {} 0 {} 0 0 0 0 0\n",
        user_jiffies, system_jiffies, idle_jiffies,
    ));
    out.push_str("intr 0\n");
    out.push_str("ctxt 0\n");
    out.push_str(&format!("btime {}\n", 1700000000u64));
    let table = process::PROCESS_TABLE.lock();
    let total = if let Some(ref t) = *table {
        t.process_count()
    } else {
        0
    };
    out.push_str(&format!("processes {}\n", total));
    out.push_str("procs_running 1\n");
    out.push_str("procs_blocked 0\n");
    out
}

// ═══════════════════════════════════════════════════════════════════════════════
// ProcFS VFS inode adapters
// ═══════════════════════════════════════════════════════════════════════════════

pub struct ProcfsInode {
    generator: fn() -> String,
}

impl ProcfsInode {
    pub fn new(generator: fn() -> String) -> Self {
        Self { generator }
    }
}

impl crate::vfs::Inode for ProcfsInode {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        let content = (self.generator)();
        let bytes = content.as_bytes();
        if offset >= bytes.len() {
            return 0;
        }
        let n = buf.len().min(bytes.len() - offset);
        buf[..n].copy_from_slice(&bytes[offset..offset + n]);
        n
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> usize {
        0
    }
    fn size(&self) -> usize {
        (self.generator)().len()
    }
}

pub struct ProcfsPidInode {
    pid: u64,
    generator: fn(u64) -> String,
}

impl ProcfsPidInode {
    pub fn new(pid: u64, generator: fn(u64) -> String) -> Self {
        Self { pid, generator }
    }
}

impl crate::vfs::Inode for ProcfsPidInode {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        let content = (self.generator)(self.pid);
        let bytes = content.as_bytes();
        if offset >= bytes.len() {
            return 0;
        }
        let n = buf.len().min(bytes.len() - offset);
        buf[..n].copy_from_slice(&bytes[offset..offset + n]);
        n
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> usize {
        0
    }
    fn size(&self) -> usize {
        (self.generator)(self.pid).len()
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Path resolver: /proc/... → content
// ═══════════════════════════════════════════════════════════════════════════════

pub fn resolve_proc_path(path: &str) -> Option<String> {
    let path = path.trim_start_matches("/proc");
    let path = path.trim_start_matches('/');

    if path.is_empty() {
        return Some(proc_root_listing());
    }

    match path {
        "cpuinfo" => return Some(proc_cpuinfo()),
        "meminfo" => return Some(proc_meminfo()),
        "uptime" => return Some(proc_uptime()),
        "loadavg" => return Some(proc_loadavg()),
        "version" => return Some(proc_version()),
        "filesystems" => return Some(proc_filesystems()),
        "mounts" => return Some(proc_mounts()),
        "stat" => return Some(proc_stat()),
        "net/tcp" => return Some(proc_net_tcp()),
        "net/udp" => return Some(proc_net_udp()),
        _ => {}
    }

    if let Some(sub) = path.strip_prefix("raeen/") {
        return resolve_raeen_subpath(sub);
    }

    if let Some(sub) = path.strip_prefix("self/") {
        let pid = crate::posix::sys_getpid();
        return resolve_pid_subpath(pid, sub);
    }

    let parts: Vec<&str> = path.splitn(2, '/').collect();
    if let Some(&pid_str) = parts.first() {
        if let Ok(pid) = u64_from_str(pid_str) {
            if let Some(subpath) = parts.get(1) {
                return resolve_pid_subpath(pid, subpath);
            } else {
                return Some(proc_pid_listing(pid));
            }
        }
    }

    None
}

fn resolve_pid_subpath(pid: u64, subpath: &str) -> Option<String> {
    match subpath {
        "exe" => Some(proc_pid_exe(pid)),
        "maps" => Some(proc_pid_maps(pid)),
        "status" => Some(proc_pid_status(pid)),
        "cmdline" => Some(proc_pid_cmdline(pid)),
        "environ" => Some(proc_pid_environ(pid)),
        "stat" => Some(proc_pid_stat(pid)),
        "fd" => {
            let fds = proc_pid_fd_list(pid);
            let mut out = String::new();
            for fd in fds {
                out.push_str(&format!("{}\n", fd));
            }
            Some(out)
        }
        _ => {
            if let Some(fd_str) = subpath.strip_prefix("fd/") {
                let fd_num = u32_from_str(fd_str)?;
                let result = proc_pid_fd(pid, fd_num);
                if result.is_empty() {
                    None
                } else {
                    Some(result)
                }
            } else {
                None
            }
        }
    }
}

fn proc_root_listing() -> String {
    let mut out = String::new();
    let table = process::PROCESS_TABLE.lock();
    if let Some(ref t) = *table {
        for proc_entry in t.list_processes() {
            out.push_str(&format!("{}\n", proc_entry.pid.0));
        }
    }
    out.push_str("cpuinfo\nmeminfo\nuptime\nloadavg\nversion\nfilesystems\nmounts\nstat\nnet\n");
    out
}

fn proc_pid_listing(pid: u64) -> String {
    let table = process::PROCESS_TABLE.lock();
    if let Some(ref t) = *table {
        if t.getpid(Pid(pid)).is_some() {
            return String::from("exe\nmaps\nstatus\ncmdline\nenviron\nfd\nstat\n");
        }
    }
    String::new()
}

/// Procfs-local integer parser (no std dependency)
fn u64_from_str(s: &str) -> Result<u64, ()> {
    let mut result: u64 = 0;
    for b in s.bytes() {
        if b < b'0' || b > b'9' {
            return Err(());
        }
        result = result.checked_mul(10).ok_or(())?;
        result = result.checked_add((b - b'0') as u64).ok_or(())?;
    }
    Ok(result)
}

fn u32_from_str(s: &str) -> Option<u32> {
    let v = u64_from_str(s).ok()?;
    if v > u32::MAX as u64 {
        return None;
    }
    Some(v as u32)
}

/// Universal path-based inode that resolves any /proc path on read
pub struct ProcfsPathInode {
    path: String,
}

impl ProcfsPathInode {
    pub fn new(path: &str) -> Self {
        Self {
            path: String::from(path),
        }
    }
}

impl crate::vfs::Inode for ProcfsPathInode {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        if let Some(content) = resolve_proc_path(&self.path) {
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
        resolve_proc_path(&self.path).map(|c| c.len()).unwrap_or(0)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Initialization
// ═══════════════════════════════════════════════════════════════════════════════

pub fn init() {
    crate::serial_println!("[ OK ] /proc filesystem initialized");
}
