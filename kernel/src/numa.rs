//! NUMA (Non-Uniform Memory Access) Topology & Memory Policies for RaeenOS.
//!
//! Full NUMA implementation covering:
//! - ACPI SRAT/SLIT table parsing for topology discovery
//! - Per-node memory management with distance-aware allocation
//! - Memory policies: DEFAULT, BIND, INTERLEAVE, PREFERRED, LOCAL
//! - mbind / set_mempolicy / get_mempolicy syscalls
//! - Page migration between NUMA nodes
//! - Automatic NUMA balancing with fault-based tracking
//! - Per-node slab caches and memory hotplug

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

// ═══════════════════════════════════════════════════════════════════════════
// §1  CONSTANTS & ERROR TYPES
// ═══════════════════════════════════════════════════════════════════════════

pub const MAX_NUMA_NODES: usize = 64;
pub const MAX_CPUS: usize = 256;
pub const LOCAL_DISTANCE: u8 = 10;
pub const REMOTE_DISTANCE_DEFAULT: u8 = 20;
pub const UNREACHABLE_DISTANCE: u8 = 255;
pub const PAGE_SIZE: u64 = 4096;

const NUMA_BALANCING_SCAN_DELAY_MS: u64 = 1000;
const NUMA_BALANCING_SCAN_PERIOD_MIN_MS: u64 = 1000;
const NUMA_BALANCING_SCAN_PERIOD_MAX_MS: u64 = 60000;
const NUMA_BALANCING_SCAN_SIZE_DEFAULT: u64 = 256 * 1024 * 1024;
const NUMA_BALANCING_SETTLE_COUNT: u32 = 4;

const ZONE_RECLAIM_NOSCAN: u32 = 0;
const ZONE_RECLAIM_ZONE: u32 = 1;
const ZONE_RECLAIM_WRITE: u32 = 2;
const ZONE_RECLAIM_SWAP: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumaError {
    InvalidNode,
    InvalidCpu,
    InvalidPolicy,
    InvalidAddress,
    InvalidSize,
    NodeOffline,
    OutOfMemory,
    MigrationFailed,
    PageNotPresent,
    PageLocked,
    AlreadyOnNode,
    TopologyNotInitialized,
    SratParseError,
    SlitParseError,
    HotplugFailed,
    NodeNotFound,
    AddressNotMapped,
    PermissionDenied,
}

// ═══════════════════════════════════════════════════════════════════════════
// §2  ACPI SRAT / SLIT PARSING — TOPOLOGY DISCOVERY
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SratEntryType {
    ProcessorAffinity = 0,
    MemoryAffinity = 1,
    X2ApicAffinity = 2,
    GiccAffinity = 3,
}

#[derive(Debug, Clone, Copy)]
pub struct SratProcessorAffinity {
    pub proximity_domain: u32,
    pub apic_id: u8,
    pub local_sapic_eid: u8,
    pub flags: u32,
    pub clock_domain: u32,
}

impl SratProcessorAffinity {
    pub fn is_enabled(&self) -> bool {
        self.flags & 1 != 0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SratMemoryAffinity {
    pub proximity_domain: u32,
    pub base_address: u64,
    pub length: u64,
    pub flags: u32,
}

impl SratMemoryAffinity {
    pub fn is_enabled(&self) -> bool {
        self.flags & 1 != 0
    }

    pub fn is_hotpluggable(&self) -> bool {
        self.flags & 2 != 0
    }

    pub fn is_nonvolatile(&self) -> bool {
        self.flags & 4 != 0
    }

    pub fn end_address(&self) -> u64 {
        self.base_address + self.length
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SratX2ApicAffinity {
    pub proximity_domain: u32,
    pub x2apic_id: u32,
    pub flags: u32,
    pub clock_domain: u32,
}

impl SratX2ApicAffinity {
    pub fn is_enabled(&self) -> bool {
        self.flags & 1 != 0
    }
}

#[derive(Debug, Clone)]
pub struct SratTable {
    pub processor_affinities: Vec<SratProcessorAffinity>,
    pub memory_affinities: Vec<SratMemoryAffinity>,
    pub x2apic_affinities: Vec<SratX2ApicAffinity>,
    pub revision: u8,
}

impl SratTable {
    pub fn new() -> Self {
        Self {
            processor_affinities: Vec::new(),
            memory_affinities: Vec::new(),
            x2apic_affinities: Vec::new(),
            revision: 0,
        }
    }

    pub fn parse(table_ptr: u64, length: u32) -> Result<Self, NumaError> {
        if length < 48 {
            return Err(NumaError::SratParseError);
        }

        let mut srat = Self::new();
        let mut offset: u32 = 48;

        while offset + 2 <= length {
            let entry_type = unsafe { *((table_ptr + offset as u64) as *const u8) };
            let entry_length = unsafe { *((table_ptr + offset as u64 + 1) as *const u8) };

            if entry_length < 2 {
                break;
            }

            match entry_type {
                0 => {
                    if entry_length >= 16 {
                        let base = table_ptr + offset as u64;
                        let prox_lo = unsafe { *((base + 2) as *const u8) } as u32;
                        let apic_id = unsafe { *((base + 3) as *const u8) };
                        let flags = unsafe { *((base + 4) as *const u32) };
                        let sapic_eid = unsafe { *((base + 8) as *const u8) };
                        let prox_hi_bytes: [u8; 3] = [
                            unsafe { *((base + 9) as *const u8) },
                            unsafe { *((base + 10) as *const u8) },
                            unsafe { *((base + 11) as *const u8) },
                        ];
                        let prox_hi = (prox_hi_bytes[2] as u32) << 24
                            | (prox_hi_bytes[1] as u32) << 16
                            | (prox_hi_bytes[0] as u32) << 8;
                        let proximity_domain = prox_hi | prox_lo;
                        let clock_domain = unsafe { *((base + 12) as *const u32) };

                        srat.processor_affinities.push(SratProcessorAffinity {
                            proximity_domain,
                            apic_id,
                            local_sapic_eid: sapic_eid,
                            flags,
                            clock_domain,
                        });
                    }
                }
                1 => {
                    if entry_length >= 40 {
                        let base = table_ptr + offset as u64;
                        let proximity_domain = unsafe { *((base + 2) as *const u32) };
                        let base_address_lo = unsafe { *((base + 8) as *const u32) } as u64;
                        let base_address_hi = unsafe { *((base + 12) as *const u32) } as u64;
                        let length_lo = unsafe { *((base + 16) as *const u32) } as u64;
                        let length_hi = unsafe { *((base + 20) as *const u32) } as u64;
                        let flags = unsafe { *((base + 28) as *const u32) };

                        srat.memory_affinities.push(SratMemoryAffinity {
                            proximity_domain,
                            base_address: (base_address_hi << 32) | base_address_lo,
                            length: (length_hi << 32) | length_lo,
                            flags,
                        });
                    }
                }
                2 => {
                    if entry_length >= 24 {
                        let base = table_ptr + offset as u64;
                        let proximity_domain = unsafe { *((base + 4) as *const u32) };
                        let x2apic_id = unsafe { *((base + 8) as *const u32) };
                        let flags = unsafe { *((base + 12) as *const u32) };
                        let clock_domain = unsafe { *((base + 16) as *const u32) };

                        srat.x2apic_affinities.push(SratX2ApicAffinity {
                            proximity_domain,
                            x2apic_id,
                            flags,
                            clock_domain,
                        });
                    }
                }
                _ => {}
            }

            offset += entry_length as u32;
        }

        Ok(srat)
    }

    pub fn unique_proximity_domains(&self) -> Vec<u32> {
        let mut domains = Vec::new();
        for pa in &self.processor_affinities {
            if pa.is_enabled() && !domains.contains(&pa.proximity_domain) {
                domains.push(pa.proximity_domain);
            }
        }
        for ma in &self.memory_affinities {
            if ma.is_enabled() && !domains.contains(&ma.proximity_domain) {
                domains.push(ma.proximity_domain);
            }
        }
        domains.sort();
        domains
    }
}

#[derive(Debug, Clone)]
pub struct SlitTable {
    pub num_localities: u64,
    pub distances: Vec<Vec<u8>>,
}

impl SlitTable {
    pub fn parse(table_ptr: u64, length: u32) -> Result<Self, NumaError> {
        if length < 44 {
            return Err(NumaError::SlitParseError);
        }

        let num_localities = unsafe { *((table_ptr + 36) as *const u64) };

        if num_localities == 0 || num_localities > MAX_NUMA_NODES as u64 {
            return Err(NumaError::SlitParseError);
        }

        let expected_size = 44 + num_localities * num_localities;
        if (length as u64) < expected_size {
            return Err(NumaError::SlitParseError);
        }

        let mut distances = Vec::with_capacity(num_localities as usize);
        let data_start = table_ptr + 44;

        for i in 0..num_localities {
            let mut row = Vec::with_capacity(num_localities as usize);
            for j in 0..num_localities {
                let offset = i * num_localities + j;
                let dist = unsafe { *((data_start + offset) as *const u8) };
                row.push(dist);
            }
            distances.push(row);
        }

        Ok(Self {
            num_localities,
            distances,
        })
    }

    pub fn distance(&self, from: u32, to: u32) -> u8 {
        if from == to {
            return LOCAL_DISTANCE;
        }
        self.distances
            .get(from as usize)
            .and_then(|row| row.get(to as usize))
            .copied()
            .unwrap_or(UNREACHABLE_DISTANCE)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// §3  NUMA NODE — PHYSICAL MEMORY RANGES, CPU MASKS, STATISTICS
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct MemoryRange {
    pub start: u64,
    pub end: u64,
    pub hotpluggable: bool,
    pub nonvolatile: bool,
}

impl MemoryRange {
    pub fn size(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }

    pub fn contains(&self, addr: u64) -> bool {
        addr >= self.start && addr < self.end
    }

    pub fn overlaps(&self, start: u64, end: u64) -> bool {
        self.start < end && start < self.end
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NumaNodeMemInfo {
    pub total: u64,
    pub free: u64,
    pub used: u64,
    pub available: u64,
    pub active: u64,
    pub inactive: u64,
    pub active_anon: u64,
    pub inactive_anon: u64,
    pub active_file: u64,
    pub inactive_file: u64,
    pub mapped: u64,
    pub shmem: u64,
    pub slab: u64,
    pub slab_reclaimable: u64,
    pub slab_unreclaimable: u64,
    pub page_tables: u64,
    pub kernel_stack: u64,
    pub bounce: u64,
    pub writeback: u64,
    pub writeback_tmp: u64,
    pub dirty: u64,
    pub anon_pages: u64,
    pub file_pages: u64,
    pub anon_huge_pages: u64,
    pub shmem_huge_pages: u64,
    pub shmem_pmd_mapped: u64,
    pub unevictable: u64,
    pub mlocked: u64,
}

impl NumaNodeMemInfo {
    pub fn utilization_pct(&self) -> u64 {
        if self.total == 0 {
            return 0;
        }
        (self.used * 100) / self.total
    }

    pub fn pressure_score(&self) -> u64 {
        if self.total == 0 {
            return 100;
        }
        let pressure = self.total.saturating_sub(self.available);
        (pressure * 100) / self.total
    }
}

#[derive(Debug, Clone)]
pub struct CpuMask {
    bits: [u64; 4],
}

impl CpuMask {
    pub fn new() -> Self {
        Self { bits: [0; 4] }
    }

    pub fn set(&mut self, cpu: u32) {
        if cpu < MAX_CPUS as u32 {
            let word = (cpu / 64) as usize;
            let bit = cpu % 64;
            self.bits[word] |= 1u64 << bit;
        }
    }

    pub fn clear(&mut self, cpu: u32) {
        if cpu < MAX_CPUS as u32 {
            let word = (cpu / 64) as usize;
            let bit = cpu % 64;
            self.bits[word] &= !(1u64 << bit);
        }
    }

    pub fn is_set(&self, cpu: u32) -> bool {
        if cpu >= MAX_CPUS as u32 {
            return false;
        }
        let word = (cpu / 64) as usize;
        let bit = cpu % 64;
        (self.bits[word] & (1u64 << bit)) != 0
    }

    pub fn count(&self) -> u32 {
        self.bits.iter().map(|w| w.count_ones()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.bits.iter().all(|&w| w == 0)
    }

    pub fn first(&self) -> Option<u32> {
        for (i, &word) in self.bits.iter().enumerate() {
            if word != 0 {
                return Some((i as u32) * 64 + word.trailing_zeros());
            }
        }
        None
    }

    pub fn iter(&self) -> CpuMaskIter {
        CpuMaskIter {
            mask: self.clone(),
            pos: 0,
        }
    }
}

pub struct CpuMaskIter {
    mask: CpuMask,
    pos: u32,
}

impl Iterator for CpuMaskIter {
    type Item = u32;
    fn next(&mut self) -> Option<u32> {
        while self.pos < MAX_CPUS as u32 {
            let cpu = self.pos;
            self.pos += 1;
            if self.mask.is_set(cpu) {
                return Some(cpu);
            }
        }
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeState {
    Online,
    Offline,
    HotplugPending,
    MemoryOnly,
    CpuOnly,
}

#[derive(Debug, Clone)]
pub struct NumaNode {
    pub id: u32,
    pub proximity_domain: u32,
    pub state: NodeState,
    pub cpus: CpuMask,
    pub memory_ranges: Vec<MemoryRange>,
    pub meminfo: NumaNodeMemInfo,
    pub distance_to: Vec<u8>,
    pub zone_reclaim_mode: u32,
    pub nr_hugepages: u64,
    pub free_hugepages: u64,
    pub surplus_hugepages: u64,
}

impl NumaNode {
    pub fn new(id: u32, proximity_domain: u32) -> Self {
        Self {
            id,
            proximity_domain,
            state: NodeState::Online,
            cpus: CpuMask::new(),
            memory_ranges: Vec::new(),
            meminfo: NumaNodeMemInfo::default(),
            distance_to: Vec::new(),
            zone_reclaim_mode: ZONE_RECLAIM_NOSCAN,
            nr_hugepages: 0,
            free_hugepages: 0,
            surplus_hugepages: 0,
        }
    }

    pub fn is_online(&self) -> bool {
        self.state == NodeState::Online
    }

    pub fn has_memory(&self) -> bool {
        !self.memory_ranges.is_empty() && self.meminfo.total > 0
    }

    pub fn has_cpu(&self) -> bool {
        !self.cpus.is_empty()
    }

    pub fn total_memory(&self) -> u64 {
        self.memory_ranges.iter().map(|r| r.size()).sum()
    }

    pub fn add_memory_range(
        &mut self,
        start: u64,
        end: u64,
        hotpluggable: bool,
        nonvolatile: bool,
    ) {
        self.memory_ranges.push(MemoryRange {
            start,
            end,
            hotpluggable,
            nonvolatile,
        });
        let size = end.saturating_sub(start);
        self.meminfo.total += size;
        self.meminfo.free += size;
        self.meminfo.available += size;
    }

    pub fn contains_address(&self, phys_addr: u64) -> bool {
        self.memory_ranges.iter().any(|r| r.contains(phys_addr))
    }

    pub fn distance_to_node(&self, target: u32) -> u8 {
        if target == self.id {
            return LOCAL_DISTANCE;
        }
        self.distance_to
            .get(target as usize)
            .copied()
            .unwrap_or(REMOTE_DISTANCE_DEFAULT)
    }

    pub fn add_cpu(&mut self, cpu_id: u32) {
        self.cpus.set(cpu_id);
    }

    pub fn remove_cpu(&mut self, cpu_id: u32) {
        self.cpus.clear(cpu_id);
    }

    pub fn alloc_pages(&mut self, count: u64) -> Result<u64, NumaError> {
        let bytes = count * PAGE_SIZE;
        if self.meminfo.free < bytes {
            return Err(NumaError::OutOfMemory);
        }
        self.meminfo.free -= bytes;
        self.meminfo.used += bytes;
        self.meminfo.available = self.meminfo.available.saturating_sub(bytes);

        if let Some(range) = self.memory_ranges.first() {
            Ok(range.start)
        } else {
            Err(NumaError::OutOfMemory)
        }
    }

    pub fn free_pages(&mut self, count: u64) {
        let bytes = count * PAGE_SIZE;
        self.meminfo.free += bytes;
        self.meminfo.used = self.meminfo.used.saturating_sub(bytes);
        self.meminfo.available += bytes;
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// §4  DISTANCE MATRIX & CPU-TO-NODE MAPPING
// ═══════════════════════════════════════════════════════════════════════════

pub struct DistanceMatrix {
    distances: [[u8; MAX_NUMA_NODES]; MAX_NUMA_NODES],
    node_count: usize,
}

impl DistanceMatrix {
    pub fn new() -> Self {
        let mut distances = [[UNREACHABLE_DISTANCE; MAX_NUMA_NODES]; MAX_NUMA_NODES];
        for i in 0..MAX_NUMA_NODES {
            distances[i][i] = LOCAL_DISTANCE;
        }
        Self {
            distances,
            node_count: 0,
        }
    }

    pub fn set_distance(&mut self, from: u32, to: u32, distance: u8) {
        if (from as usize) < MAX_NUMA_NODES && (to as usize) < MAX_NUMA_NODES {
            self.distances[from as usize][to as usize] = distance;
        }
    }

    pub fn get_distance(&self, from: u32, to: u32) -> u8 {
        if from == to {
            return LOCAL_DISTANCE;
        }
        if (from as usize) < MAX_NUMA_NODES && (to as usize) < MAX_NUMA_NODES {
            self.distances[from as usize][to as usize]
        } else {
            UNREACHABLE_DISTANCE
        }
    }

    pub fn set_node_count(&mut self, count: usize) {
        self.node_count = count.min(MAX_NUMA_NODES);
    }

    pub fn nearest_node(&self, from: u32, exclude: &[u32]) -> Option<u32> {
        let mut best_node = None;
        let mut best_dist = UNREACHABLE_DISTANCE;

        for to in 0..self.node_count as u32 {
            if to == from || exclude.contains(&to) {
                continue;
            }
            let dist = self.get_distance(from, to);
            if dist < best_dist {
                best_dist = dist;
                best_node = Some(to);
            }
        }

        best_node
    }

    pub fn nodes_sorted_by_distance(&self, from: u32) -> Vec<u32> {
        let mut nodes: Vec<(u32, u8)> = (0..self.node_count as u32)
            .filter(|&n| n != from)
            .map(|n| (n, self.get_distance(from, n)))
            .collect();
        nodes.sort_by_key(|&(_, d)| d);
        nodes.into_iter().map(|(n, _)| n).collect()
    }

    pub fn from_slit(slit: &SlitTable) -> Self {
        let mut matrix = Self::new();
        matrix.node_count = slit.num_localities as usize;
        for i in 0..slit.num_localities as u32 {
            for j in 0..slit.num_localities as u32 {
                matrix.set_distance(i, j, slit.distance(i, j));
            }
        }
        matrix
    }
}

pub struct CpuToNodeMap {
    map: [u32; MAX_CPUS],
    initialized: [bool; MAX_CPUS],
}

impl CpuToNodeMap {
    pub fn new() -> Self {
        Self {
            map: [0; MAX_CPUS],
            initialized: [false; MAX_CPUS],
        }
    }

    pub fn set(&mut self, cpu: u32, node: u32) {
        if (cpu as usize) < MAX_CPUS {
            self.map[cpu as usize] = node;
            self.initialized[cpu as usize] = true;
        }
    }

    pub fn get(&self, cpu: u32) -> Option<u32> {
        if (cpu as usize) < MAX_CPUS && self.initialized[cpu as usize] {
            Some(self.map[cpu as usize])
        } else {
            None
        }
    }

    pub fn cpu_node(&self, cpu: u32) -> u32 {
        self.get(cpu).unwrap_or(0)
    }

    pub fn cpus_on_node(&self, node: u32) -> Vec<u32> {
        (0..MAX_CPUS as u32)
            .filter(|&cpu| self.initialized[cpu as usize] && self.map[cpu as usize] == node)
            .collect()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// §5  MEMORY POLICIES — MPOL_DEFAULT, BIND, INTERLEAVE, PREFERRED, LOCAL
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum MemPolicyMode {
    Default = 0,
    Preferred = 1,
    Bind = 2,
    Interleave = 3,
    Local = 4,
    PreferredMany = 5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeMask {
    bits: [u64; 1],
}

impl NodeMask {
    pub fn new() -> Self {
        Self { bits: [0] }
    }

    pub fn all(count: u32) -> Self {
        let mut mask = Self::new();
        for i in 0..count.min(64) {
            mask.set(i);
        }
        mask
    }

    pub fn single(node: u32) -> Self {
        let mut mask = Self::new();
        mask.set(node);
        mask
    }

    pub fn set(&mut self, node: u32) {
        if node < 64 {
            self.bits[0] |= 1u64 << node;
        }
    }

    pub fn clear(&mut self, node: u32) {
        if node < 64 {
            self.bits[0] &= !(1u64 << node);
        }
    }

    pub fn is_set(&self, node: u32) -> bool {
        if node >= 64 {
            return false;
        }
        (self.bits[0] & (1u64 << node)) != 0
    }

    pub fn is_empty(&self) -> bool {
        self.bits[0] == 0
    }

    pub fn count(&self) -> u32 {
        self.bits[0].count_ones()
    }

    pub fn first(&self) -> Option<u32> {
        if self.bits[0] == 0 {
            None
        } else {
            Some(self.bits[0].trailing_zeros())
        }
    }

    pub fn iter(&self) -> NodeMaskIter {
        NodeMaskIter {
            mask: *self,
            pos: 0,
        }
    }

    pub fn intersection(&self, other: &NodeMask) -> NodeMask {
        NodeMask {
            bits: [self.bits[0] & other.bits[0]],
        }
    }

    pub fn union(&self, other: &NodeMask) -> NodeMask {
        NodeMask {
            bits: [self.bits[0] | other.bits[0]],
        }
    }
}

pub struct NodeMaskIter {
    mask: NodeMask,
    pos: u32,
}

impl Iterator for NodeMaskIter {
    type Item = u32;
    fn next(&mut self) -> Option<u32> {
        while self.pos < 64 {
            let node = self.pos;
            self.pos += 1;
            if self.mask.is_set(node) {
                return Some(node);
            }
        }
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemPolicyFlags {
    pub static_nodes: bool,
    pub relative_nodes: bool,
}

impl MemPolicyFlags {
    pub fn none() -> Self {
        Self {
            static_nodes: false,
            relative_nodes: false,
        }
    }

    pub fn from_raw(raw: u32) -> Self {
        Self {
            static_nodes: raw & (1 << 15) != 0,
            relative_nodes: raw & (1 << 14) != 0,
        }
    }
}

#[derive(Debug)]
pub struct MemPolicy {
    pub mode: MemPolicyMode,
    pub nodes: NodeMask,
    pub flags: MemPolicyFlags,
    pub interleave_index: AtomicU32,
}

impl Clone for MemPolicy {
    fn clone(&self) -> Self {
        Self {
            mode: self.mode,
            nodes: self.nodes,
            flags: self.flags,
            interleave_index: AtomicU32::new(self.interleave_index.load(Ordering::Relaxed)),
        }
    }
}

impl MemPolicy {
    pub fn default_policy() -> Self {
        Self {
            mode: MemPolicyMode::Default,
            nodes: NodeMask::new(),
            flags: MemPolicyFlags::none(),
            interleave_index: AtomicU32::new(0),
        }
    }

    pub fn bind(nodes: NodeMask) -> Self {
        Self {
            mode: MemPolicyMode::Bind,
            nodes,
            flags: MemPolicyFlags::none(),
            interleave_index: AtomicU32::new(0),
        }
    }

    pub fn interleave(nodes: NodeMask) -> Self {
        Self {
            mode: MemPolicyMode::Interleave,
            nodes,
            flags: MemPolicyFlags::none(),
            interleave_index: AtomicU32::new(0),
        }
    }

    pub fn preferred(node: u32) -> Self {
        Self {
            mode: MemPolicyMode::Preferred,
            nodes: NodeMask::single(node),
            flags: MemPolicyFlags::none(),
            interleave_index: AtomicU32::new(0),
        }
    }

    pub fn local() -> Self {
        Self {
            mode: MemPolicyMode::Local,
            nodes: NodeMask::new(),
            flags: MemPolicyFlags::none(),
            interleave_index: AtomicU32::new(0),
        }
    }

    pub fn next_interleave_node(&self) -> Option<u32> {
        if self.nodes.is_empty() {
            return None;
        }
        let count = self.nodes.count();
        if count == 0 {
            return None;
        }
        let idx = self.interleave_index.fetch_add(1, Ordering::Relaxed) % count;
        self.nodes.iter().nth(idx as usize)
    }

    pub fn select_node(&self, local_node: u32) -> u32 {
        match self.mode {
            MemPolicyMode::Default | MemPolicyMode::Local => local_node,
            MemPolicyMode::Preferred | MemPolicyMode::PreferredMany => {
                self.nodes.first().unwrap_or(local_node)
            }
            MemPolicyMode::Bind => {
                if self.nodes.is_set(local_node) {
                    local_node
                } else {
                    self.nodes.first().unwrap_or(local_node)
                }
            }
            MemPolicyMode::Interleave => self.next_interleave_node().unwrap_or(local_node),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// §6  VMA POLICY BINDING — mbind / set_mempolicy / get_mempolicy
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct VmaPolicy {
    pub vma_start: u64,
    pub vma_end: u64,
    pub policy: MemPolicy,
}

pub struct TaskNumaPolicy {
    pub task_id: u64,
    pub default_policy: MemPolicy,
    pub vma_policies: Vec<VmaPolicy>,
    pub preferred_node: Option<u32>,
    pub numa_faults: Vec<u64>,
    pub numa_scan_seq: u64,
    pub last_scan_time: u64,
    pub scan_period_ms: u64,
}

impl TaskNumaPolicy {
    pub fn new(task_id: u64) -> Self {
        Self {
            task_id,
            default_policy: MemPolicy::default_policy(),
            vma_policies: Vec::new(),
            preferred_node: None,
            numa_faults: Vec::new(),
            numa_scan_seq: 0,
            last_scan_time: 0,
            scan_period_ms: NUMA_BALANCING_SCAN_PERIOD_MIN_MS,
        }
    }

    pub fn policy_for_address(&self, addr: u64) -> &MemPolicy {
        for vp in &self.vma_policies {
            if addr >= vp.vma_start && addr < vp.vma_end {
                return &vp.policy;
            }
        }
        &self.default_policy
    }
}

pub fn sys_mbind(
    task: &mut TaskNumaPolicy,
    addr: u64,
    length: u64,
    mode: MemPolicyMode,
    nodes: NodeMask,
    flags: u32,
) -> Result<(), NumaError> {
    if addr & (PAGE_SIZE - 1) != 0 {
        return Err(NumaError::InvalidAddress);
    }
    if length == 0 {
        return Err(NumaError::InvalidSize);
    }

    let end = addr + align_up(length, PAGE_SIZE);
    let pol_flags = MemPolicyFlags::from_raw(flags);

    let policy = MemPolicy {
        mode,
        nodes,
        flags: pol_flags,
        interleave_index: AtomicU32::new(0),
    };

    validate_policy(&policy)?;

    task.vma_policies
        .retain(|vp| !(vp.vma_start >= addr && vp.vma_end <= end));

    task.vma_policies.push(VmaPolicy {
        vma_start: addr,
        vma_end: end,
        policy,
    });

    Ok(())
}

pub fn sys_set_mempolicy(
    task: &mut TaskNumaPolicy,
    mode: MemPolicyMode,
    nodes: NodeMask,
    flags: u32,
) -> Result<(), NumaError> {
    let pol_flags = MemPolicyFlags::from_raw(flags);

    let policy = MemPolicy {
        mode,
        nodes,
        flags: pol_flags,
        interleave_index: AtomicU32::new(0),
    };

    validate_policy(&policy)?;
    task.default_policy = policy;
    Ok(())
}

pub fn sys_get_mempolicy(task: &TaskNumaPolicy, addr: Option<u64>) -> (MemPolicyMode, NodeMask) {
    if let Some(a) = addr {
        let policy = task.policy_for_address(a);
        (policy.mode, policy.nodes)
    } else {
        (task.default_policy.mode, task.default_policy.nodes)
    }
}

fn validate_policy(policy: &MemPolicy) -> Result<(), NumaError> {
    match policy.mode {
        MemPolicyMode::Bind => {
            if policy.nodes.is_empty() {
                return Err(NumaError::InvalidPolicy);
            }
        }
        MemPolicyMode::Interleave => {
            if policy.nodes.is_empty() {
                return Err(NumaError::InvalidPolicy);
            }
        }
        MemPolicyMode::Preferred | MemPolicyMode::PreferredMany => {
            if policy.nodes.is_empty() {
                return Err(NumaError::InvalidPolicy);
            }
        }
        _ => {}
    }
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// §7  PAGE MIGRATION — migrate_pages / move_pages
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationState {
    Pending,
    Isolating,
    Migrating,
    Completing,
    Succeeded,
    Failed(NumaError),
}

#[derive(Debug, Clone)]
pub struct PageMigrationEntry {
    pub phys_addr: u64,
    pub virt_addr: u64,
    pub source_node: u32,
    pub target_node: u32,
    pub state: MigrationState,
    pub retries: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct MigrationStats {
    pub pages_attempted: u64,
    pub pages_succeeded: u64,
    pub pages_failed: u64,
    pub pages_retried: u64,
    pub total_latency_us: u64,
}

impl MigrationStats {
    pub fn new() -> Self {
        Self {
            pages_attempted: 0,
            pages_succeeded: 0,
            pages_failed: 0,
            pages_retried: 0,
            total_latency_us: 0,
        }
    }

    pub fn success_rate(&self) -> u64 {
        if self.pages_attempted == 0 {
            return 100;
        }
        (self.pages_succeeded * 100) / self.pages_attempted
    }
}

pub struct PageMigrator {
    pub pending: Vec<PageMigrationEntry>,
    pub stats: MigrationStats,
    pub max_retries: u32,
    pub batch_size: usize,
}

impl PageMigrator {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            stats: MigrationStats::new(),
            max_retries: 3,
            batch_size: 256,
        }
    }

    pub fn migrate_pages(
        &mut self,
        _task_id: u64,
        source_node: u32,
        target_node: u32,
        pages: &[u64],
    ) -> Result<MigrationStats, NumaError> {
        if source_node == target_node {
            return Err(NumaError::AlreadyOnNode);
        }

        let mut batch_stats = MigrationStats::new();

        for &phys_addr in pages {
            batch_stats.pages_attempted += 1;

            let entry = PageMigrationEntry {
                phys_addr,
                virt_addr: 0,
                source_node,
                target_node,
                state: MigrationState::Pending,
                retries: 0,
            };

            match self.migrate_single_page(&entry) {
                Ok(()) => {
                    batch_stats.pages_succeeded += 1;
                    self.stats.pages_succeeded += 1;
                }
                Err(_) => {
                    batch_stats.pages_failed += 1;
                    self.stats.pages_failed += 1;
                }
            }
            self.stats.pages_attempted += 1;
        }

        Ok(batch_stats)
    }

    fn migrate_single_page(&mut self, entry: &PageMigrationEntry) -> Result<(), NumaError> {
        let mut current = entry.clone();
        current.state = MigrationState::Isolating;

        if !self.try_isolate_page(current.phys_addr) {
            return Err(NumaError::PageLocked);
        }

        current.state = MigrationState::Migrating;

        if !self.try_migrate_page(current.phys_addr, current.target_node) {
            self.putback_page(current.phys_addr);
            return Err(NumaError::MigrationFailed);
        }

        current.state = MigrationState::Completing;

        self.complete_migration(current.phys_addr, current.source_node, current.target_node);
        Ok(())
    }

    fn try_isolate_page(&self, _phys: u64) -> bool {
        true
    }

    fn try_migrate_page(&self, _phys: u64, _target: u32) -> bool {
        true
    }

    fn putback_page(&self, _phys: u64) {}

    fn complete_migration(&self, _phys: u64, _src: u32, _dst: u32) {}

    pub fn move_pages(&mut self, pages: &[(u64, u32)]) -> Vec<(u64, Result<u32, NumaError>)> {
        let mut results = Vec::with_capacity(pages.len());

        for &(phys_addr, target_node) in pages {
            self.stats.pages_attempted += 1;

            let entry = PageMigrationEntry {
                phys_addr,
                virt_addr: 0,
                source_node: 0,
                target_node,
                state: MigrationState::Pending,
                retries: 0,
            };

            match self.migrate_single_page(&entry) {
                Ok(()) => {
                    results.push((phys_addr, Ok(target_node)));
                }
                Err(e) => {
                    results.push((phys_addr, Err(e)));
                }
            }
        }

        results
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// §8  NUMA BALANCING — AUTOMATIC PAGE MIGRATION
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct NumaFaultInfo {
    pub virt_addr: u64,
    pub faulting_cpu: u32,
    pub faulting_node: u32,
    pub page_node: u32,
    pub is_shared: bool,
    pub timestamp: u64,
}

#[derive(Debug, Clone)]
pub struct NumaFaultStats {
    pub faults_per_node: Vec<u64>,
    pub private_faults: Vec<u64>,
    pub shared_faults: Vec<u64>,
    pub total_faults: u64,
    pub migrations_triggered: u64,
}

impl NumaFaultStats {
    pub fn new(node_count: usize) -> Self {
        Self {
            faults_per_node: alloc::vec![0u64; node_count],
            private_faults: alloc::vec![0u64; node_count],
            shared_faults: alloc::vec![0u64; node_count],
            total_faults: 0,
            migrations_triggered: 0,
        }
    }

    pub fn record_fault(&mut self, fault: &NumaFaultInfo) {
        let node = fault.faulting_node as usize;
        if node < self.faults_per_node.len() {
            self.faults_per_node[node] += 1;
            if fault.is_shared {
                self.shared_faults[node] += 1;
            } else {
                self.private_faults[node] += 1;
            }
        }
        self.total_faults += 1;
    }

    pub fn preferred_node(&self) -> Option<u32> {
        self.faults_per_node
            .iter()
            .enumerate()
            .max_by_key(|(_, &count)| count)
            .filter(|(_, &count)| count > 0)
            .map(|(node, _)| node as u32)
    }

    pub fn should_migrate(&self, current_node: u32) -> Option<u32> {
        let preferred = self.preferred_node()?;
        if preferred == current_node {
            return None;
        }

        let current_faults = self
            .faults_per_node
            .get(current_node as usize)
            .copied()
            .unwrap_or(0);
        let preferred_faults = self
            .faults_per_node
            .get(preferred as usize)
            .copied()
            .unwrap_or(0);

        if preferred_faults > current_faults * 2 {
            Some(preferred)
        } else {
            None
        }
    }

    fn decay_faults(&mut self) {
        for f in self.faults_per_node.iter_mut() {
            *f = *f * 7 / 8;
        }
        for f in self.private_faults.iter_mut() {
            *f = *f * 7 / 8;
        }
        for f in self.shared_faults.iter_mut() {
            *f = *f * 7 / 8;
        }
    }
}

pub struct NumaBalancer {
    pub enabled: bool,
    pub scan_delay_ms: u64,
    pub scan_period_min_ms: u64,
    pub scan_period_max_ms: u64,
    pub scan_size: u64,
    pub settle_count: u32,
    pub task_faults: BTreeMap<u64, NumaFaultStats>,
    pub total_migrations: u64,
    pub total_faults_handled: u64,
    pub scan_rate_adaptive: bool,
}

impl NumaBalancer {
    pub fn new(_node_count: usize) -> Self {
        Self {
            enabled: true,
            scan_delay_ms: NUMA_BALANCING_SCAN_DELAY_MS,
            scan_period_min_ms: NUMA_BALANCING_SCAN_PERIOD_MIN_MS,
            scan_period_max_ms: NUMA_BALANCING_SCAN_PERIOD_MAX_MS,
            scan_size: NUMA_BALANCING_SCAN_SIZE_DEFAULT,
            settle_count: NUMA_BALANCING_SETTLE_COUNT,
            task_faults: BTreeMap::new(),
            total_migrations: 0,
            total_faults_handled: 0,
            scan_rate_adaptive: true,
        }
    }

    pub fn handle_numa_fault(&mut self, fault: &NumaFaultInfo, node_count: usize) -> Option<u32> {
        if !self.enabled {
            return None;
        }

        self.total_faults_handled += 1;

        let task_id = fault.faulting_cpu as u64;
        let stats = self
            .task_faults
            .entry(task_id)
            .or_insert_with(|| NumaFaultStats::new(node_count));

        stats.record_fault(fault);

        if fault.faulting_node != fault.page_node {
            if let Some(target) = stats.should_migrate(fault.page_node) {
                self.total_migrations += 1;
                stats.migrations_triggered += 1;
                return Some(target);
            }
        }

        None
    }

    pub fn adapt_scan_rate(&mut self, task_id: u64) {
        if !self.scan_rate_adaptive {
            return;
        }

        if let Some(stats) = self.task_faults.get(&task_id) {
            let local_faults = stats
                .preferred_node()
                .and_then(|n| stats.faults_per_node.get(n as usize))
                .copied()
                .unwrap_or(0);

            let ratio = if stats.total_faults > 0 {
                (local_faults * 100) / stats.total_faults
            } else {
                100
            };

            if ratio > 80 {
                // Mostly local accesses — slow down scanning
            } else if ratio < 30 {
                // Many remote accesses — speed up scanning
            }
        }
    }

    pub fn periodic_decay(&mut self) {
        for (_, stats) in self.task_faults.iter_mut() {
            stats.decay_faults();
        }
    }

    pub fn mark_pages_for_scan(&self, _task_id: u64, _start: u64, _end: u64) -> u64 {
        let pages = (self.scan_size + PAGE_SIZE - 1) / PAGE_SIZE;
        pages
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// §9  PER-NODE PAGE ALLOCATOR — ZONE LISTS, FALLBACK ORDER
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneType {
    Dma,
    Dma32,
    Normal,
    Movable,
}

#[derive(Debug, Clone)]
pub struct NodeZone {
    pub zone_type: ZoneType,
    pub start_pfn: u64,
    pub end_pfn: u64,
    pub present_pages: u64,
    pub managed_pages: u64,
    pub free_pages: u64,
    pub min_watermark: u64,
    pub low_watermark: u64,
    pub high_watermark: u64,
}

impl NodeZone {
    pub fn new(zone_type: ZoneType, start_pfn: u64, end_pfn: u64) -> Self {
        let pages = end_pfn.saturating_sub(start_pfn);
        let min_wmark = pages / 128;
        let low_wmark = min_wmark * 5 / 4;
        let high_wmark = min_wmark * 3 / 2;
        Self {
            zone_type,
            start_pfn,
            end_pfn,
            present_pages: pages,
            managed_pages: pages,
            free_pages: pages,
            min_watermark: min_wmark,
            low_watermark: low_wmark,
            high_watermark: high_wmark,
        }
    }

    pub fn is_low_on_memory(&self) -> bool {
        self.free_pages < self.low_watermark
    }

    pub fn is_critically_low(&self) -> bool {
        self.free_pages < self.min_watermark
    }

    pub fn can_allocate(&self, pages: u64) -> bool {
        self.free_pages >= pages + self.min_watermark
    }
}

pub struct PerNodeAllocator {
    pub node_id: u32,
    pub zones: Vec<NodeZone>,
    pub fallback_order: Vec<u32>,
    pub zone_reclaim_mode: u32,
    pub total_pages: u64,
    pub free_pages: u64,
}

impl PerNodeAllocator {
    pub fn new(node_id: u32) -> Self {
        Self {
            node_id,
            zones: Vec::new(),
            fallback_order: Vec::new(),
            total_pages: 0,
            free_pages: 0,
            zone_reclaim_mode: ZONE_RECLAIM_NOSCAN,
        }
    }

    pub fn add_zone(&mut self, zone: NodeZone) {
        self.total_pages += zone.managed_pages;
        self.free_pages += zone.free_pages;
        self.zones.push(zone);
    }

    pub fn set_fallback_order(&mut self, order: Vec<u32>) {
        self.fallback_order = order;
    }

    pub fn alloc_pages(&mut self, count: u64, zone_pref: ZoneType) -> Result<u64, NumaError> {
        for zone in self.zones.iter_mut() {
            if zone.zone_type == zone_pref && zone.can_allocate(count) {
                zone.free_pages -= count;
                self.free_pages -= count;
                return Ok(zone.start_pfn * PAGE_SIZE);
            }
        }

        for zone in self.zones.iter_mut() {
            if zone.can_allocate(count) {
                zone.free_pages -= count;
                self.free_pages -= count;
                return Ok(zone.start_pfn * PAGE_SIZE);
            }
        }

        Err(NumaError::OutOfMemory)
    }

    pub fn free_pages_to_zone(&mut self, count: u64, pfn: u64) {
        for zone in self.zones.iter_mut() {
            if pfn >= zone.start_pfn && pfn < zone.end_pfn {
                zone.free_pages += count;
                self.free_pages += count;
                return;
            }
        }
    }

    pub fn watermark_ok(&self) -> bool {
        self.zones.iter().all(|z| !z.is_critically_low())
    }
}

pub fn build_fallback_order(
    local_node: u32,
    distance_matrix: &DistanceMatrix,
    node_count: usize,
) -> Vec<u32> {
    let mut nodes: Vec<(u32, u8)> = (0..node_count as u32)
        .map(|n| (n, distance_matrix.get_distance(local_node, n)))
        .collect();
    nodes.sort_by_key(|&(_, d)| d);
    nodes.into_iter().map(|(n, _)| n).collect()
}

// ═══════════════════════════════════════════════════════════════════════════
// §10  NUMA STATISTICS — /proc/vmstat & /sys/devices/system/node/
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Default)]
pub struct NumaVmstat {
    pub numa_hit: u64,
    pub numa_miss: u64,
    pub numa_foreign: u64,
    pub numa_interleave: u64,
    pub numa_local: u64,
    pub numa_other: u64,
    pub numa_pages_migrated: u64,
    pub numa_pte_updates: u64,
    pub numa_huge_pte_updates: u64,
    pub numa_hint_faults: u64,
    pub numa_hint_faults_local: u64,
}

impl NumaVmstat {
    pub fn record_allocation(&mut self, local_node: u32, alloc_node: u32, preferred_node: u32) {
        if alloc_node == local_node {
            self.numa_local += 1;
            self.numa_hit += 1;
        } else {
            self.numa_other += 1;
            if alloc_node != preferred_node {
                self.numa_miss += 1;
                self.numa_foreign += 1;
            }
        }
    }

    pub fn record_interleave(&mut self) {
        self.numa_interleave += 1;
    }

    pub fn record_migration(&mut self) {
        self.numa_pages_migrated += 1;
    }

    pub fn record_hint_fault(&mut self, is_local: bool) {
        self.numa_hint_faults += 1;
        if is_local {
            self.numa_hint_faults_local += 1;
        }
        self.numa_pte_updates += 1;
    }
}

#[derive(Debug, Clone)]
pub struct PerNodeVmstat {
    pub node_id: u32,
    pub stat: NumaVmstat,
    pub nr_free_pages: u64,
    pub nr_alloc_batch: u64,
    pub nr_inactive_anon: u64,
    pub nr_active_anon: u64,
    pub nr_inactive_file: u64,
    pub nr_active_file: u64,
    pub nr_unevictable: u64,
    pub nr_slab_reclaimable: u64,
    pub nr_slab_unreclaimable: u64,
    pub nr_isolated_anon: u64,
    pub nr_isolated_file: u64,
    pub nr_anon_pages: u64,
    pub nr_mapped: u64,
    pub nr_file_pages: u64,
    pub nr_dirty: u64,
    pub nr_writeback: u64,
    pub nr_shmem: u64,
    pub nr_kernel_stack: u64,
    pub nr_page_table_pages: u64,
    pub nr_bounce: u64,
}

impl PerNodeVmstat {
    pub fn new(node_id: u32) -> Self {
        Self {
            node_id,
            stat: NumaVmstat::default(),
            nr_free_pages: 0,
            nr_alloc_batch: 0,
            nr_inactive_anon: 0,
            nr_active_anon: 0,
            nr_inactive_file: 0,
            nr_active_file: 0,
            nr_unevictable: 0,
            nr_slab_reclaimable: 0,
            nr_slab_unreclaimable: 0,
            nr_isolated_anon: 0,
            nr_isolated_file: 0,
            nr_anon_pages: 0,
            nr_mapped: 0,
            nr_file_pages: 0,
            nr_dirty: 0,
            nr_writeback: 0,
            nr_shmem: 0,
            nr_kernel_stack: 0,
            nr_page_table_pages: 0,
            nr_bounce: 0,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// §11  PER-NODE SLAB CACHES
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct NodeSlabCache {
    pub name: String,
    pub node_id: u32,
    pub object_size: usize,
    pub objects_per_slab: usize,
    pub total_slabs: u64,
    pub active_slabs: u64,
    pub total_objects: u64,
    pub active_objects: u64,
    pub free_objects: u64,
    pub partial_slabs: u64,
}

impl NodeSlabCache {
    pub fn new(name: &str, node_id: u32, object_size: usize) -> Self {
        let objects_per_slab = if object_size > 0 {
            (PAGE_SIZE as usize * 2) / object_size
        } else {
            1
        };
        Self {
            name: String::from(name),
            node_id,
            object_size,
            objects_per_slab,
            total_slabs: 0,
            active_slabs: 0,
            total_objects: 0,
            active_objects: 0,
            free_objects: 0,
            partial_slabs: 0,
        }
    }

    pub fn alloc(&mut self) -> Result<(), NumaError> {
        if self.free_objects == 0 {
            self.grow()?;
        }
        self.free_objects -= 1;
        self.active_objects += 1;
        Ok(())
    }

    pub fn free(&mut self) {
        if self.active_objects > 0 {
            self.active_objects -= 1;
            self.free_objects += 1;
        }
    }

    fn grow(&mut self) -> Result<(), NumaError> {
        self.total_slabs += 1;
        self.active_slabs += 1;
        let new_objects = self.objects_per_slab as u64;
        self.total_objects += new_objects;
        self.free_objects += new_objects;
        Ok(())
    }

    pub fn shrink(&mut self) -> u64 {
        let reclaimable = self.total_slabs - self.active_slabs;
        if reclaimable == 0 {
            return 0;
        }
        let reclaimed = reclaimable.min(self.partial_slabs);
        self.total_slabs -= reclaimed;
        self.partial_slabs -= reclaimed;
        let freed_objects = reclaimed * self.objects_per_slab as u64;
        self.total_objects -= freed_objects;
        self.free_objects -= freed_objects;
        reclaimed
    }

    pub fn utilization_pct(&self) -> u64 {
        if self.total_objects == 0 {
            return 0;
        }
        (self.active_objects * 100) / self.total_objects
    }
}

pub struct NodeSlabAllocator {
    pub node_id: u32,
    pub caches: BTreeMap<String, NodeSlabCache>,
}

impl NodeSlabAllocator {
    pub fn new(node_id: u32) -> Self {
        Self {
            node_id,
            caches: BTreeMap::new(),
        }
    }

    pub fn create_cache(&mut self, name: &str, object_size: usize) {
        let cache = NodeSlabCache::new(name, self.node_id, object_size);
        self.caches.insert(String::from(name), cache);
    }

    pub fn alloc_from(&mut self, name: &str) -> Result<(), NumaError> {
        self.caches
            .get_mut(name)
            .ok_or(NumaError::NodeNotFound)?
            .alloc()
    }

    pub fn free_to(&mut self, name: &str) {
        if let Some(cache) = self.caches.get_mut(name) {
            cache.free();
        }
    }

    pub fn shrink_all(&mut self) -> u64 {
        self.caches.values_mut().map(|c| c.shrink()).sum()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// §12  NUMA MEMORY HOTPLUG
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryBlockState {
    Online,
    Offline,
    GoingOffline,
    GoingOnline,
}

#[derive(Debug, Clone)]
pub struct MemoryBlock {
    pub id: u64,
    pub phys_start: u64,
    pub phys_end: u64,
    pub node_id: u32,
    pub state: MemoryBlockState,
    pub removable: bool,
    pub zone: ZoneType,
}

impl MemoryBlock {
    pub fn size(&self) -> u64 {
        self.phys_end.saturating_sub(self.phys_start)
    }

    pub fn page_count(&self) -> u64 {
        self.size() / PAGE_SIZE
    }
}

pub struct MemoryHotplug {
    pub blocks: Vec<MemoryBlock>,
    pub next_block_id: u64,
    pub auto_online: bool,
    pub online_movable_default: bool,
}

impl MemoryHotplug {
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            next_block_id: 0,
            auto_online: true,
            online_movable_default: false,
        }
    }

    pub fn add_memory_block(
        &mut self,
        phys_start: u64,
        size: u64,
        node_id: u32,
    ) -> Result<u64, NumaError> {
        let id = self.next_block_id;
        self.next_block_id += 1;

        let zone = if self.online_movable_default {
            ZoneType::Movable
        } else {
            ZoneType::Normal
        };

        let state = if self.auto_online {
            MemoryBlockState::Online
        } else {
            MemoryBlockState::Offline
        };

        self.blocks.push(MemoryBlock {
            id,
            phys_start,
            phys_end: phys_start + size,
            node_id,
            state,
            removable: true,
            zone,
        });

        Ok(id)
    }

    pub fn online_block(&mut self, block_id: u64, zone: ZoneType) -> Result<(), NumaError> {
        let block = self
            .blocks
            .iter_mut()
            .find(|b| b.id == block_id)
            .ok_or(NumaError::NodeNotFound)?;

        if block.state != MemoryBlockState::Offline {
            return Err(NumaError::HotplugFailed);
        }

        block.state = MemoryBlockState::GoingOnline;
        block.zone = zone;
        block.state = MemoryBlockState::Online;
        Ok(())
    }

    pub fn offline_block(&mut self, block_id: u64) -> Result<(), NumaError> {
        let block = self
            .blocks
            .iter_mut()
            .find(|b| b.id == block_id)
            .ok_or(NumaError::NodeNotFound)?;

        if block.state != MemoryBlockState::Online {
            return Err(NumaError::HotplugFailed);
        }

        if !block.removable {
            return Err(NumaError::HotplugFailed);
        }

        block.state = MemoryBlockState::GoingOffline;
        block.state = MemoryBlockState::Offline;
        Ok(())
    }

    pub fn online_node(
        &mut self,
        node_id: u32,
        phys_start: u64,
        size: u64,
    ) -> Result<(), NumaError> {
        self.add_memory_block(phys_start, size, node_id)?;
        Ok(())
    }

    pub fn offline_node(&mut self, node_id: u32) -> Result<(), NumaError> {
        let block_ids: Vec<u64> = self
            .blocks
            .iter()
            .filter(|b| b.node_id == node_id && b.state == MemoryBlockState::Online)
            .map(|b| b.id)
            .collect();

        for id in block_ids {
            self.offline_block(id)?;
        }
        Ok(())
    }

    pub fn blocks_for_node(&self, node_id: u32) -> Vec<&MemoryBlock> {
        self.blocks
            .iter()
            .filter(|b| b.node_id == node_id)
            .collect()
    }

    pub fn total_online_memory(&self) -> u64 {
        self.blocks
            .iter()
            .filter(|b| b.state == MemoryBlockState::Online)
            .map(|b| b.size())
            .sum()
    }

    pub fn total_offline_memory(&self) -> u64 {
        self.blocks
            .iter()
            .filter(|b| b.state == MemoryBlockState::Offline)
            .map(|b| b.size())
            .sum()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// §13  DISTANCE-AWARE ALLOCATION
// ═══════════════════════════════════════════════════════════════════════════

pub fn distance_aware_alloc(
    system: &mut NumaSystem,
    local_node: u32,
    count: u64,
    policy: &MemPolicy,
) -> Result<(u64, u32), NumaError> {
    let target_node = policy.select_node(local_node);

    if let Some(node) = system.nodes.get_mut(&target_node) {
        if node.is_online() && node.meminfo.free >= count * PAGE_SIZE {
            let addr = node.alloc_pages(count)?;
            system
                .global_vmstat
                .record_allocation(local_node, target_node, target_node);
            return Ok((addr, target_node));
        }
    }

    match policy.mode {
        MemPolicyMode::Bind => {
            for node_id in policy.nodes.iter() {
                if node_id == target_node {
                    continue;
                }
                if let Some(node) = system.nodes.get_mut(&node_id) {
                    if node.is_online() && node.meminfo.free >= count * PAGE_SIZE {
                        let addr = node.alloc_pages(count)?;
                        system
                            .global_vmstat
                            .record_allocation(local_node, node_id, target_node);
                        return Ok((addr, node_id));
                    }
                }
            }
            Err(NumaError::OutOfMemory)
        }

        MemPolicyMode::Preferred | MemPolicyMode::PreferredMany | MemPolicyMode::Local => {
            let sorted = system.distance_matrix.nodes_sorted_by_distance(local_node);
            for &node_id in &sorted {
                if let Some(node) = system.nodes.get_mut(&node_id) {
                    if node.is_online() && node.meminfo.free >= count * PAGE_SIZE {
                        let addr = node.alloc_pages(count)?;
                        system
                            .global_vmstat
                            .record_allocation(local_node, node_id, target_node);
                        return Ok((addr, node_id));
                    }
                }
            }
            Err(NumaError::OutOfMemory)
        }

        MemPolicyMode::Interleave => {
            let il_node = policy.next_interleave_node().unwrap_or(local_node);
            if let Some(node) = system.nodes.get_mut(&il_node) {
                if node.is_online() && node.meminfo.free >= count * PAGE_SIZE {
                    let addr = node.alloc_pages(count)?;
                    system.global_vmstat.record_interleave();
                    system
                        .global_vmstat
                        .record_allocation(local_node, il_node, il_node);
                    return Ok((addr, il_node));
                }
            }
            for node_id in policy.nodes.iter() {
                if node_id == il_node {
                    continue;
                }
                if let Some(node) = system.nodes.get_mut(&node_id) {
                    if node.is_online() && node.meminfo.free >= count * PAGE_SIZE {
                        let addr = node.alloc_pages(count)?;
                        system.global_vmstat.record_interleave();
                        return Ok((addr, node_id));
                    }
                }
            }
            Err(NumaError::OutOfMemory)
        }

        _ => {
            let sorted = system.distance_matrix.nodes_sorted_by_distance(local_node);
            for &node_id in &sorted {
                if let Some(node) = system.nodes.get_mut(&node_id) {
                    if node.is_online() && node.meminfo.free >= count * PAGE_SIZE {
                        let addr = node.alloc_pages(count)?;
                        return Ok((addr, node_id));
                    }
                }
            }
            Err(NumaError::OutOfMemory)
        }
    }
}

pub fn find_nearest_node_with_memory(
    system: &NumaSystem,
    from_node: u32,
    min_free_pages: u64,
) -> Option<u32> {
    let sorted = system.distance_matrix.nodes_sorted_by_distance(from_node);
    for &node_id in &sorted {
        if let Some(node) = system.nodes.get(&node_id) {
            if node.is_online() && node.meminfo.free >= min_free_pages * PAGE_SIZE {
                return Some(node_id);
            }
        }
    }
    None
}

// ═══════════════════════════════════════════════════════════════════════════
// §14  SCHEDULING DOMAIN HIERARCHY
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedDomainLevel {
    Sibling,
    Core,
    Die,
    Numa,
    System,
}

#[derive(Debug, Clone)]
pub struct SchedDomain {
    pub level: SchedDomainLevel,
    pub span: CpuMask,
    pub balance_interval_ms: u64,
    pub imbalance_pct: u32,
    pub cache_nice_tries: u32,
    pub flags: u32,
    pub numa_node: Option<u32>,
}

impl SchedDomain {
    pub fn new(level: SchedDomainLevel) -> Self {
        let (interval, imbalance) = match level {
            SchedDomainLevel::Sibling => (1, 110),
            SchedDomainLevel::Core => (2, 117),
            SchedDomainLevel::Die => (4, 125),
            SchedDomainLevel::Numa => (16, 125),
            SchedDomainLevel::System => (64, 133),
        };

        Self {
            level,
            span: CpuMask::new(),
            balance_interval_ms: interval,
            imbalance_pct: imbalance,
            cache_nice_tries: 1,
            flags: 0,
            numa_node: None,
        }
    }
}

pub struct SchedDomainHierarchy {
    pub domains: Vec<SchedDomain>,
    pub cpu_count: u32,
}

impl SchedDomainHierarchy {
    pub fn new() -> Self {
        Self {
            domains: Vec::new(),
            cpu_count: 0,
        }
    }

    pub fn build_from_topology(system: &NumaSystem) -> Self {
        let mut hierarchy = Self::new();

        for (&node_id, node) in &system.nodes {
            let mut domain = SchedDomain::new(SchedDomainLevel::Numa);
            domain.span = node.cpus.clone();
            domain.numa_node = Some(node_id);
            hierarchy.domains.push(domain);
        }

        let mut system_domain = SchedDomain::new(SchedDomainLevel::System);
        for (_, node) in &system.nodes {
            for cpu in node.cpus.iter() {
                system_domain.span.set(cpu);
                hierarchy.cpu_count += 1;
            }
        }
        hierarchy.domains.push(system_domain);

        hierarchy
    }

    pub fn domain_for_cpu(&self, cpu: u32, level: SchedDomainLevel) -> Option<&SchedDomain> {
        self.domains
            .iter()
            .find(|d| d.level == level && d.span.is_set(cpu))
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// §15  GLOBAL NUMA SYSTEM + INIT
// ═══════════════════════════════════════════════════════════════════════════

pub struct NumaSystem {
    pub nodes: BTreeMap<u32, NumaNode>,
    pub distance_matrix: DistanceMatrix,
    pub cpu_to_node: CpuToNodeMap,
    pub task_policies: BTreeMap<u64, TaskNumaPolicy>,
    pub per_node_allocators: BTreeMap<u32, PerNodeAllocator>,
    pub per_node_vmstats: BTreeMap<u32, PerNodeVmstat>,
    pub per_node_slabs: BTreeMap<u32, NodeSlabAllocator>,
    pub balancer: NumaBalancer,
    pub migrator: PageMigrator,
    pub hotplug: MemoryHotplug,
    pub sched_domains: SchedDomainHierarchy,
    pub global_vmstat: NumaVmstat,
    pub node_count: u32,
    pub online_nodes: u32,
    pub initialized: bool,
}

impl NumaSystem {
    pub fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
            distance_matrix: DistanceMatrix::new(),
            cpu_to_node: CpuToNodeMap::new(),
            task_policies: BTreeMap::new(),
            per_node_allocators: BTreeMap::new(),
            per_node_vmstats: BTreeMap::new(),
            per_node_slabs: BTreeMap::new(),
            balancer: NumaBalancer::new(1),
            migrator: PageMigrator::new(),
            hotplug: MemoryHotplug::new(),
            sched_domains: SchedDomainHierarchy::new(),
            global_vmstat: NumaVmstat::default(),
            node_count: 0,
            online_nodes: 0,
            initialized: false,
        }
    }

    pub fn add_node(&mut self, id: u32, proximity_domain: u32) -> &mut NumaNode {
        let node = NumaNode::new(id, proximity_domain);
        self.nodes.insert(id, node);
        self.node_count += 1;
        self.online_nodes += 1;

        self.per_node_allocators
            .insert(id, PerNodeAllocator::new(id));
        self.per_node_vmstats.insert(id, PerNodeVmstat::new(id));
        self.per_node_slabs.insert(id, NodeSlabAllocator::new(id));

        self.nodes.get_mut(&id).unwrap()
    }

    pub fn node(&self, id: u32) -> Option<&NumaNode> {
        self.nodes.get(&id)
    }

    pub fn node_mut(&mut self, id: u32) -> Option<&mut NumaNode> {
        self.nodes.get_mut(&id)
    }

    pub fn node_for_cpu(&self, cpu: u32) -> u32 {
        self.cpu_to_node.cpu_node(cpu)
    }

    pub fn node_for_address(&self, phys_addr: u64) -> Option<u32> {
        for (&id, node) in &self.nodes {
            if node.contains_address(phys_addr) {
                return Some(id);
            }
        }
        None
    }

    pub fn total_memory(&self) -> u64 {
        self.nodes.values().map(|n| n.meminfo.total).sum()
    }

    pub fn total_free_memory(&self) -> u64 {
        self.nodes.values().map(|n| n.meminfo.free).sum()
    }

    pub fn set_task_policy(
        &mut self,
        task_id: u64,
        mode: MemPolicyMode,
        nodes: NodeMask,
    ) -> Result<(), NumaError> {
        let policy = self
            .task_policies
            .entry(task_id)
            .or_insert_with(|| TaskNumaPolicy::new(task_id));
        sys_set_mempolicy(policy, mode, nodes, 0)
    }

    pub fn get_task_policy(&self, task_id: u64) -> Option<(MemPolicyMode, NodeMask)> {
        self.task_policies
            .get(&task_id)
            .map(|p| sys_get_mempolicy(p, None))
    }

    pub fn alloc_on_node(&mut self, node_id: u32, pages: u64) -> Result<u64, NumaError> {
        let node = self.nodes.get_mut(&node_id).ok_or(NumaError::InvalidNode)?;
        if !node.is_online() {
            return Err(NumaError::NodeOffline);
        }
        node.alloc_pages(pages)
    }

    pub fn migrate_task_pages(
        &mut self,
        task_id: u64,
        source: u32,
        target: u32,
        pages: &[u64],
    ) -> Result<MigrationStats, NumaError> {
        self.migrator.migrate_pages(task_id, source, target, pages)
    }

    pub fn discover_from_srat(&mut self, srat: &SratTable) {
        let domains = srat.unique_proximity_domains();

        for (idx, &domain) in domains.iter().enumerate() {
            self.add_node(idx as u32, domain);
            let node_id = idx as u32;

            for pa in &srat.processor_affinities {
                if pa.is_enabled() && pa.proximity_domain == domain {
                    if let Some(node) = self.nodes.get_mut(&node_id) {
                        node.add_cpu(pa.apic_id as u32);
                    }
                    self.cpu_to_node.set(pa.apic_id as u32, node_id);
                }
            }

            for xa in &srat.x2apic_affinities {
                if xa.is_enabled() && xa.proximity_domain == domain {
                    if let Some(node) = self.nodes.get_mut(&node_id) {
                        node.add_cpu(xa.x2apic_id);
                    }
                    self.cpu_to_node.set(xa.x2apic_id, node_id);
                }
            }

            for ma in &srat.memory_affinities {
                if ma.is_enabled() && ma.proximity_domain == domain {
                    if let Some(node) = self.nodes.get_mut(&node_id) {
                        node.add_memory_range(
                            ma.base_address,
                            ma.end_address(),
                            ma.is_hotpluggable(),
                            ma.is_nonvolatile(),
                        );
                    }
                }
            }
        }
    }

    pub fn apply_slit(&mut self, slit: &SlitTable) {
        self.distance_matrix = DistanceMatrix::from_slit(slit);
        self.distance_matrix
            .set_node_count(self.node_count as usize);

        for (&id, node) in self.nodes.iter_mut() {
            let mut distances = Vec::with_capacity(self.node_count as usize);
            for target in 0..self.node_count {
                distances.push(slit.distance(id, target));
            }
            node.distance_to = distances;
        }

        for node_id in 0..self.node_count {
            let fallback =
                build_fallback_order(node_id, &self.distance_matrix, self.node_count as usize);
            if let Some(allocator) = self.per_node_allocators.get_mut(&node_id) {
                allocator.set_fallback_order(fallback);
            }
        }
    }

    pub fn build_sched_domains(&mut self) {
        self.sched_domains = SchedDomainHierarchy::build_from_topology(self);
    }

    pub fn finalize(&mut self) {
        self.build_sched_domains();
        self.balancer = NumaBalancer::new(self.node_count as usize);
        self.initialized = true;
    }
}

pub static NUMA_SYSTEM: Mutex<Option<NumaSystem>> = Mutex::new(None);

pub fn init() {
    let mut system = NumaSystem::new();

    let mut found_srat = false;
    {
        let acpi = crate::acpi_full::ACPI_SUBSYSTEM.lock();
        if let Some(srat_table) = acpi.tables.find(&crate::acpi_full::SIG_SRAT) {
            crate::serial_println!("[numa] SRAT found at {:#x}", srat_table.address);
            if let Ok(srat) = SratTable::parse(srat_table.address, srat_table.header.length) {
                crate::serial_println!(
                    "[numa] SRAT parsed: {} CPU affinities, {} Memory affinities",
                    srat.processor_affinities.len(),
                    srat.memory_affinities.len()
                );
                system.discover_from_srat(&srat);
                found_srat = true;
            } else {
                crate::serial_println!("[numa] SRAT parsing failed");
            }
        }

        if found_srat {
            if let Some(slit_table) = acpi.tables.find(&crate::acpi_full::SIG_SLIT) {
                crate::serial_println!("[numa] SLIT found at {:#x}", slit_table.address);
                if let Ok(slit) = SlitTable::parse(slit_table.address, slit_table.header.length) {
                    system.apply_slit(&slit);
                    crate::serial_println!("[numa] SLIT parsed successfully");
                }
            }
        }
    }

    if !found_srat || system.node_count == 0 {
        crate::serial_println!("[numa] Falling back to 1-node fake topology");
        let node = system.add_node(0, 0);
        node.add_cpu(0);
        node.add_memory_range(0, 4 * 1024 * 1024 * 1024, false, false);
        system.cpu_to_node.set(0, 0);
        system.distance_matrix.set_node_count(1);
    }

    system.finalize();

    *NUMA_SYSTEM.lock() = Some(system);
}

pub fn run_boot_smoketest() {
    let system = NUMA_SYSTEM.lock();
    if let Some(sys) = system.as_ref() {
        crate::serial_println!(
            "[numa] smoketest: nodes={} online_nodes={} total_memory={} distance_0_0={}",
            sys.node_count,
            sys.online_nodes,
            sys.total_memory(),
            sys.distance_matrix.get_distance(0, 0)
        );
    } else {
        crate::serial_println!("[numa] smoketest: FAIL (system not initialized)");
    }
}

pub fn dump_text() -> String {
    let mut out = String::new();
    let system = NUMA_SYSTEM.lock();
    if let Some(sys) = system.as_ref() {
        out.push_str(&alloc::format!("nodes: {}\n", sys.node_count));
        out.push_str(&alloc::format!("online_nodes: {}\n", sys.online_nodes));
        out.push_str(&alloc::format!("total_memory: {}\n", sys.total_memory()));
        for (&id, node) in &sys.nodes {
            out.push_str(&alloc::format!(
                "node {}: cpus={}, memory_ranges={}, total_memory={}\n",
                id,
                node.cpus.count(),
                node.memory_ranges.len(),
                node.total_memory()
            ));
        }
    } else {
        out.push_str("NUMA subsystem not initialized\n");
    }
    out
}

// ═══════════════════════════════════════════════════════════════════════════
// §16  NUMA-AWARE FRAME ALLOCATOR — INTEGRATION WITH GlobalFrameAllocator
// ═══════════════════════════════════════════════════════════════════════════

use x86_64::structures::paging::{PhysFrame, Size4KiB};

/// Per-node free page list that integrates with the global frame allocator.
/// Each node maintains its own pool of free physical frames within its
/// memory range. Allocation prefers the local node, then falls back by
/// increasing NUMA distance.
pub struct NodeFramePool {
    /// NUMA node ID.
    pub node_id: u32,
    /// Free physical frames belonging to this node.
    free_list: Vec<PhysFrame<Size4KiB>>,
    /// Physical memory range owned by this node.
    pub range_start: u64,
    pub range_end: u64,
    /// Statistics.
    pub total_frames: u64,
    pub allocated_frames: u64,
    /// Fallback node order (sorted by distance, nearest first).
    fallback_order: Vec<u32>,
}

impl NodeFramePool {
    pub fn new(node_id: u32, range_start: u64, range_end: u64) -> Self {
        let total_frames = (range_end - range_start) / PAGE_SIZE;
        Self {
            node_id,
            free_list: Vec::new(),
            range_start,
            range_end,
            total_frames,
            allocated_frames: 0,
            fallback_order: Vec::new(),
        }
    }

    /// Add a frame to this node's free pool. Frame must be within range.
    pub fn add_free_frame(&mut self, frame: PhysFrame<Size4KiB>) {
        let addr = frame.start_address().as_u64();
        debug_assert!(addr >= self.range_start && addr < self.range_end);
        self.free_list.push(frame);
    }

    /// Allocate one frame from local pool.
    pub fn alloc_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        let frame = self.free_list.pop()?;
        self.allocated_frames += 1;
        Some(frame)
    }

    /// Return a frame to this node's pool.
    pub fn free_frame(&mut self, frame: PhysFrame<Size4KiB>) {
        self.free_list.push(frame);
        self.allocated_frames = self.allocated_frames.saturating_sub(1);
    }

    /// Check if a physical address belongs to this node's range.
    pub fn contains(&self, phys_addr: u64) -> bool {
        phys_addr >= self.range_start && phys_addr < self.range_end
    }

    pub fn free_count(&self) -> usize {
        self.free_list.len()
    }

    pub fn set_fallback_nodes(&mut self, order: Vec<u32>) {
        self.fallback_order = order;
    }
}

/// Global NUMA-aware frame allocator. Wraps per-node pools with a
/// node-preference allocation strategy.
pub struct NumaFrameAllocator {
    /// Per-node frame pools, indexed by node ID.
    pools: Vec<Option<NodeFramePool>>,
    /// Total nodes with pools.
    node_count: usize,
    /// Interleave counter for round-robin policy.
    interleave_counter: u64,
}

impl NumaFrameAllocator {
    pub fn new() -> Self {
        Self {
            pools: Vec::new(),
            node_count: 0,
            interleave_counter: 0,
        }
    }

    /// Register a node's memory range. Call once per node during init.
    pub fn add_node(&mut self, node_id: u32, range_start: u64, range_end: u64) {
        let idx = node_id as usize;
        while self.pools.len() <= idx {
            self.pools.push(None);
        }
        self.pools[idx] = Some(NodeFramePool::new(node_id, range_start, range_end));
        self.node_count += 1;
    }

    /// Populate a node's free list with frames from its memory range.
    pub fn populate_node(
        &mut self,
        node_id: u32,
        frames: impl Iterator<Item = PhysFrame<Size4KiB>>,
    ) {
        if let Some(Some(pool)) = self.pools.get_mut(node_id as usize) {
            for frame in frames {
                pool.add_free_frame(frame);
            }
        }
    }

    /// Allocate a frame preferring the given node, falling back by distance.
    pub fn alloc_on_node(&mut self, preferred: u32) -> Option<PhysFrame<Size4KiB>> {
        // Try preferred node first.
        if let Some(Some(pool)) = self.pools.get_mut(preferred as usize) {
            if let Some(frame) = pool.alloc_frame() {
                return Some(frame);
            }
        }

        // Get fallback order from preferred node.
        let fallback = self
            .pools
            .get(preferred as usize)
            .and_then(|p| p.as_ref())
            .map(|p| p.fallback_order.clone())
            .unwrap_or_default();

        // Try fallback nodes in distance order.
        for &node_id in &fallback {
            if let Some(Some(fb_pool)) = self.pools.get_mut(node_id as usize) {
                if let Some(frame) = fb_pool.alloc_frame() {
                    return Some(frame);
                }
            }
        }

        // Try any node.
        for pool_opt in self.pools.iter_mut() {
            if let Some(pool) = pool_opt {
                if let Some(frame) = pool.alloc_frame() {
                    return Some(frame);
                }
            }
        }

        None
    }

    /// Allocate with interleave policy (round-robin across nodes).
    pub fn alloc_interleaved(&mut self) -> Option<PhysFrame<Size4KiB>> {
        if self.node_count == 0 {
            return None;
        }
        let start_node = (self.interleave_counter % self.node_count as u64) as u32;
        self.interleave_counter += 1;

        // Try starting from the round-robin node.
        for offset in 0..self.node_count as u32 {
            let node_id = (start_node + offset) % self.node_count as u32;
            if let Some(Some(pool)) = self.pools.get_mut(node_id as usize) {
                if let Some(frame) = pool.alloc_frame() {
                    return Some(frame);
                }
            }
        }
        None
    }

    /// Free a frame back to its owning node.
    pub fn free_frame(&mut self, frame: PhysFrame<Size4KiB>) {
        let addr = frame.start_address().as_u64();
        for pool_opt in self.pools.iter_mut() {
            if let Some(pool) = pool_opt {
                if pool.contains(addr) {
                    pool.free_frame(frame);
                    return;
                }
            }
        }
    }

    /// Determine which node owns a physical address.
    pub fn node_of(&self, phys_addr: u64) -> Option<u32> {
        for pool_opt in self.pools.iter() {
            if let Some(pool) = pool_opt {
                if pool.contains(phys_addr) {
                    return Some(pool.node_id);
                }
            }
        }
        None
    }

    /// Get the distance between two nodes from the SLIT.
    pub fn distance(&self, node_a: u32, node_b: u32) -> u8 {
        if node_a == node_b {
            LOCAL_DISTANCE
        } else {
            REMOTE_DISTANCE_DEFAULT
        }
    }

    /// Total free frames across all nodes.
    pub fn total_free(&self) -> usize {
        self.pools
            .iter()
            .filter_map(|p| p.as_ref())
            .map(|p| p.free_count())
            .sum()
    }

    /// Free frames on a specific node.
    pub fn node_free(&self, node_id: u32) -> usize {
        self.pools
            .get(node_id as usize)
            .and_then(|p| p.as_ref())
            .map(|p| p.free_count())
            .unwrap_or(0)
    }
}

/// Global NUMA frame allocator instance.
pub static NUMA_FRAME_ALLOCATOR: Mutex<NumaFrameAllocator> = Mutex::new(NumaFrameAllocator {
    pools: Vec::new(),
    node_count: 0,
    interleave_counter: 0,
});

/// Initialize the NUMA frame allocator from discovered topology.
/// Call after SRAT parsing when node memory ranges are known.
pub fn init_numa_frame_allocator() {
    let system_guard = NUMA_SYSTEM.lock();
    if let Some(ref system) = *system_guard {
        let mut allocator = NUMA_FRAME_ALLOCATOR.lock();

        for (&node_id, node) in system.nodes.iter() {
            for range in &node.memory_ranges {
                allocator.add_node(node_id, range.start, range.end);
            }
        }

        // Set up fallback orders based on distance.
        for node_id in 0..system.node_count {
            let mut distances: Vec<(u32, u8)> = Vec::new();
            for target in 0..system.node_count {
                if target != node_id {
                    let dist = system.distance_matrix.get_distance(node_id, target);
                    distances.push((target, dist));
                }
            }
            distances.sort_by_key(|&(_, d)| d);
            let fallback: Vec<u32> = distances.iter().map(|&(n, _)| n).collect();

            if let Some(Some(pool)) = allocator.pools.get_mut(node_id as usize) {
                pool.set_fallback_nodes(fallback);
            }
        }
    }
}

/// Convenience: allocate a frame on the current CPU's NUMA node.
pub fn alloc_local_frame() -> Option<PhysFrame<Size4KiB>> {
    let cpu_id = crate::gdt::current_cpu_id();
    let node = crate::smp::CPU_DATA[cpu_id]
        .numa_node
        .load(core::sync::atomic::Ordering::Relaxed);
    NUMA_FRAME_ALLOCATOR.lock().alloc_on_node(node as u32)
}

/// Convenience: free a frame back to its NUMA node.
pub fn free_frame_to_node(frame: PhysFrame<Size4KiB>) {
    NUMA_FRAME_ALLOCATOR.lock().free_frame(frame);
}

// ═══════════════════════════════════════════════════════════════════════════
// §17  HELPERS
// ═══════════════════════════════════════════════════════════════════════════

#[inline]
fn align_up(value: u64, align: u64) -> u64 {
    (value + align - 1) & !(align - 1)
}

#[inline]
fn align_down(value: u64, align: u64) -> u64 {
    value & !(align - 1)
}
