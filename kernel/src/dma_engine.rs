#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// DMA direction & types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmaDirection {
    MemToMem,
    MemToDev,
    DevToMem,
    DevToDev,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmaTransferType {
    Memcpy,
    Slave,
    Cyclic,
    Interleaved,
    Memset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmaAddrWidth {
    Width1Byte,
    Width2Bytes,
    Width4Bytes,
    Width8Bytes,
    Width16Bytes,
    Width32Bytes,
}

impl DmaAddrWidth {
    pub fn bytes(&self) -> usize {
        match self {
            Self::Width1Byte => 1,
            Self::Width2Bytes => 2,
            Self::Width4Bytes => 4,
            Self::Width8Bytes => 8,
            Self::Width16Bytes => 16,
            Self::Width32Bytes => 32,
        }
    }
}

// ---------------------------------------------------------------------------
// DMA channel
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelState {
    Idle,
    InProgress,
    Paused,
    Completed,
    Error,
}

pub struct DmaChannelConfig {
    pub direction: DmaDirection,
    pub src_addr: u64,
    pub dst_addr: u64,
    pub src_addr_width: DmaAddrWidth,
    pub dst_addr_width: DmaAddrWidth,
    pub src_burst_len: u32,
    pub dst_burst_len: u32,
    pub src_maxburst: u32,
    pub dst_maxburst: u32,
    pub device_fc: bool,
}

impl DmaChannelConfig {
    pub fn new() -> Self {
        Self {
            direction: DmaDirection::MemToMem,
            src_addr: 0,
            dst_addr: 0,
            src_addr_width: DmaAddrWidth::Width4Bytes,
            dst_addr_width: DmaAddrWidth::Width4Bytes,
            src_burst_len: 1,
            dst_burst_len: 1,
            src_maxburst: 16,
            dst_maxburst: 16,
            device_fc: false,
        }
    }
}

pub struct DmaChannel {
    pub id: u32,
    pub name: String,
    pub state: ChannelState,
    pub config: DmaChannelConfig,
    pub phys_channel: Option<u32>,
    pub cookie_counter: u64,
    pub completed_cookie: u64,
    pub private_data: u64,
}

impl DmaChannel {
    fn new(id: u32, name: &str) -> Self {
        Self {
            id,
            name: String::from(name),
            state: ChannelState::Idle,
            config: DmaChannelConfig::new(),
            phys_channel: None,
            cookie_counter: 1,
            completed_cookie: 0,
            private_data: 0,
        }
    }

    fn next_cookie(&mut self) -> u64 {
        let c = self.cookie_counter;
        self.cookie_counter += 1;
        c
    }
}

// ---------------------------------------------------------------------------
// Scatter-gather list
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ScatterEntry {
    pub page_addr: u64,
    pub offset: u32,
    pub length: u32,
    pub dma_address: u64,
    pub dma_length: u32,
}

impl ScatterEntry {
    pub fn new(page_addr: u64, offset: u32, length: u32) -> Self {
        Self {
            page_addr,
            offset,
            length,
            dma_address: 0,
            dma_length: 0,
        }
    }
}

pub struct Scatterlist {
    pub entries: Vec<ScatterEntry>,
}

impl Scatterlist {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn add(&mut self, page_addr: u64, offset: u32, length: u32) {
        self.entries
            .push(ScatterEntry::new(page_addr, offset, length));
    }

    pub fn nents(&self) -> usize {
        self.entries.len()
    }

    pub fn total_length(&self) -> u64 {
        self.entries.iter().map(|e| e.length as u64).sum()
    }
}

// ---------------------------------------------------------------------------
// DMA mapping
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmaMappingType {
    Streaming,
    Coherent,
}

#[derive(Debug, Clone)]
struct DmaMapping {
    id: u64,
    cpu_addr: u64,
    dma_addr: u64,
    size: usize,
    direction: DmaDirection,
    mapping_type: DmaMappingType,
    bounce: bool,
}

pub fn dma_map_single(cpu_addr: u64, size: usize, dir: DmaDirection) -> u64 {
    let dma_addr = cpu_addr;
    let mut engine = DMA_ENGINE.lock();
    if let Some(engine) = engine.as_mut() {
        let mapping = DmaMapping {
            id: engine.next_mapping_id(),
            cpu_addr,
            dma_addr,
            size,
            direction: dir,
            mapping_type: DmaMappingType::Streaming,
            bounce: false,
        };
        engine.active_mappings.push(mapping);
        engine.debug.map_count.fetch_add(1, Ordering::Relaxed);
    }
    dma_addr
}

pub fn dma_unmap_single(dma_addr: u64, size: usize, _dir: DmaDirection) {
    let mut engine = DMA_ENGINE.lock();
    if let Some(engine) = engine.as_mut() {
        engine.active_mappings.retain(|m| {
            !(m.dma_addr == dma_addr
                && m.size == size
                && m.mapping_type == DmaMappingType::Streaming)
        });
        engine.debug.unmap_count.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn dma_map_sg(sg: &mut Scatterlist, dir: DmaDirection) -> usize {
    let mut mapped = 0;
    let mut engine = DMA_ENGINE.lock();
    if let Some(engine) = engine.as_mut() {
        for entry in &mut sg.entries {
            let cpu_addr = entry.page_addr + entry.offset as u64;
            let dma_addr = apply_iommu_translation(engine, cpu_addr);
            entry.dma_address = dma_addr;
            entry.dma_length = entry.length;
            let mapping_id = engine.next_mapping_id();
            engine.active_mappings.push(DmaMapping {
                id: mapping_id,
                cpu_addr,
                dma_addr,
                size: entry.length as usize,
                direction: dir,
                mapping_type: DmaMappingType::Streaming,
                bounce: false,
            });
            mapped += 1;
        }
        engine
            .debug
            .map_count
            .fetch_add(mapped as u64, Ordering::Relaxed);
    }
    mapped
}

pub fn dma_unmap_sg(sg: &Scatterlist, _dir: DmaDirection) {
    let mut engine = DMA_ENGINE.lock();
    if let Some(engine) = engine.as_mut() {
        for entry in &sg.entries {
            engine
                .active_mappings
                .retain(|m| m.dma_addr != entry.dma_address);
        }
        engine
            .debug
            .unmap_count
            .fetch_add(sg.nents() as u64, Ordering::Relaxed);
    }
}

pub fn dma_map_page(page_addr: u64, offset: usize, size: usize, dir: DmaDirection) -> u64 {
    dma_map_single(page_addr + offset as u64, size, dir)
}

fn apply_iommu_translation(engine: &DmaEngine, cpu_addr: u64) -> u64 {
    for domain in &engine.iommu_domains {
        if domain.bypass {
            return cpu_addr;
        }
        for mapping in &domain.translations {
            if cpu_addr >= mapping.iova && cpu_addr < mapping.iova + mapping.size {
                return mapping.phys + (cpu_addr - mapping.iova);
            }
        }
    }
    cpu_addr
}

// ---------------------------------------------------------------------------
// DMA coherent allocations
// ---------------------------------------------------------------------------

pub struct DmaCoherentAlloc {
    pub cpu_addr: u64,
    pub dma_addr: u64,
    pub size: usize,
}

pub fn dma_alloc_coherent(size: usize) -> Option<DmaCoherentAlloc> {
    let aligned_size = (size + 0xFFF) & !0xFFF;
    let mut engine = DMA_ENGINE.lock();
    if let Some(engine) = engine.as_mut() {
        // BUG-28: first-fit a freed range (rangemap splits a larger range and
        // merges adjacent frees), else bump. A pure bump pointer leaked the
        // coherent address space on every alloc/free cycle.
        let want = aligned_size as u64;
        let reuse = engine
            .coherent_free
            .iter()
            .find(|r| r.end - r.start >= want)
            .map(|r| r.start);
        let addr = if let Some(start) = reuse {
            engine.coherent_free.remove(start..start + want);
            start
        } else {
            let a = engine.coherent_next_addr;
            engine.coherent_next_addr += want;
            a
        };
        let alloc = DmaCoherentAlloc {
            cpu_addr: addr,
            dma_addr: addr,
            size: aligned_size,
        };
        let mapping_id = engine.next_mapping_id();
        engine.active_mappings.push(DmaMapping {
            id: mapping_id,
            cpu_addr: addr,
            dma_addr: addr,
            size: aligned_size,
            direction: DmaDirection::MemToMem,
            mapping_type: DmaMappingType::Coherent,
            bounce: false,
        });
        Some(alloc)
    } else {
        None
    }
}

pub fn dma_free_coherent(alloc: &DmaCoherentAlloc) {
    let mut engine = DMA_ENGINE.lock();
    if let Some(engine) = engine.as_mut() {
        let removed = {
            let before = engine.active_mappings.len();
            engine.active_mappings.retain(|m| {
                !(m.cpu_addr == alloc.cpu_addr
                    && m.mapping_type == DmaMappingType::Coherent
                    && m.size == alloc.size)
            });
            engine.active_mappings.len() != before
        };
        // BUG-28: return the range to the interval set so the address space is
        // reclaimed (rangemap merges it with any adjacent free ranges). Only if
        // it was a live mapping — guards double-free.
        if removed {
            let start = alloc.cpu_addr;
            engine
                .coherent_free
                .insert(start..start + alloc.size as u64);
        }
    }
}

// ---------------------------------------------------------------------------
// DMA pool
// ---------------------------------------------------------------------------

pub struct DmaPool {
    pub name: String,
    pub element_size: usize,
    pub align: usize,
    pub boundary: usize,
    free_list: Vec<DmaPoolEntry>,
    allocated: Vec<DmaPoolEntry>,
    pool_base: u64,
    pool_size: usize,
    next_offset: usize,
}

#[derive(Debug, Clone)]
struct DmaPoolEntry {
    cpu_addr: u64,
    dma_addr: u64,
    size: usize,
}

impl DmaPool {
    pub fn create(name: &str, element_size: usize, align: usize, boundary: usize) -> Self {
        let pool_base = {
            let mut engine = DMA_ENGINE.lock();
            if let Some(e) = engine.as_mut() {
                let base = e.pool_next_addr;
                e.pool_next_addr += 0x10000;
                base
            } else {
                0x3000_0000
            }
        };
        Self {
            name: String::from(name),
            element_size: core::cmp::max(element_size, 8),
            align: core::cmp::max(align, 4),
            boundary: if boundary == 0 { 4096 } else { boundary },
            free_list: Vec::new(),
            allocated: Vec::new(),
            pool_base,
            pool_size: 0x10000,
            next_offset: 0,
        }
    }

    pub fn alloc(&mut self) -> Option<(u64, u64)> {
        if let Some(entry) = self.free_list.pop() {
            let cpu = entry.cpu_addr;
            let dma = entry.dma_addr;
            self.allocated.push(entry);
            return Some((cpu, dma));
        }
        let aligned_off = (self.next_offset + self.align - 1) & !(self.align - 1);
        if aligned_off + self.element_size > self.pool_size {
            return None;
        }
        let cpu_addr = self.pool_base + aligned_off as u64;
        let dma_addr = cpu_addr;
        self.next_offset = aligned_off + self.element_size;
        let entry = DmaPoolEntry {
            cpu_addr,
            dma_addr,
            size: self.element_size,
        };
        self.allocated.push(entry);
        Some((cpu_addr, dma_addr))
    }

    pub fn free(&mut self, cpu_addr: u64, dma_addr: u64) {
        if let Some(idx) = self.allocated.iter().position(|e| e.cpu_addr == cpu_addr) {
            let entry = self.allocated.remove(idx);
            self.free_list.push(entry);
        } else {
            let _ = dma_addr;
        }
    }

    pub fn destroy(self) {
        // Pool memory is not backed by real allocations in this stub,
        // so nothing to release.
    }
}

// ---------------------------------------------------------------------------
// DMA descriptors & async_tx
// ---------------------------------------------------------------------------

pub type DmaCookie = u64;

pub type DmaCallback = Box<dyn FnOnce(DmaCookie) + Send>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DescriptorStatus {
    Pending,
    InProgress,
    Completed,
    Error,
}

pub struct DmaDescriptor {
    pub cookie: DmaCookie,
    pub tx_type: DmaTransferType,
    pub src_addr: u64,
    pub dst_addr: u64,
    pub length: usize,
    pub status: DescriptorStatus,
    pub callback: Option<DmaCallback>,
    pub sg_list: Option<Scatterlist>,
    pub period_len: usize,
    pub num_periods: usize,
    pub pattern: u8,
}

impl DmaDescriptor {
    fn new_memcpy(cookie: DmaCookie, src: u64, dst: u64, len: usize) -> Self {
        Self {
            cookie,
            tx_type: DmaTransferType::Memcpy,
            src_addr: src,
            dst_addr: dst,
            length: len,
            status: DescriptorStatus::Pending,
            callback: None,
            sg_list: None,
            period_len: 0,
            num_periods: 0,
            pattern: 0,
        }
    }

    fn new_slave(
        cookie: DmaCookie,
        dev_addr: u64,
        mem_addr: u64,
        len: usize,
        dir: DmaDirection,
    ) -> Self {
        let (src, dst) = match dir {
            DmaDirection::DevToMem => (dev_addr, mem_addr),
            _ => (mem_addr, dev_addr),
        };
        Self {
            cookie,
            tx_type: DmaTransferType::Slave,
            src_addr: src,
            dst_addr: dst,
            length: len,
            status: DescriptorStatus::Pending,
            callback: None,
            sg_list: None,
            period_len: 0,
            num_periods: 0,
            pattern: 0,
        }
    }

    fn new_cyclic(
        cookie: DmaCookie,
        buf_addr: u64,
        buf_len: usize,
        period_len: usize,
        dir: DmaDirection,
        dev_addr: u64,
    ) -> Self {
        let (src, dst) = match dir {
            DmaDirection::DevToMem => (dev_addr, buf_addr),
            _ => (buf_addr, dev_addr),
        };
        Self {
            cookie,
            tx_type: DmaTransferType::Cyclic,
            src_addr: src,
            dst_addr: dst,
            length: buf_len,
            status: DescriptorStatus::Pending,
            callback: None,
            sg_list: None,
            period_len,
            num_periods: if period_len > 0 {
                buf_len / period_len
            } else {
                0
            },
            pattern: 0,
        }
    }

    fn new_memset(cookie: DmaCookie, dst: u64, pattern: u8, len: usize) -> Self {
        Self {
            cookie,
            tx_type: DmaTransferType::Memset,
            src_addr: 0,
            dst_addr: dst,
            length: len,
            status: DescriptorStatus::Pending,
            callback: None,
            sg_list: None,
            period_len: 0,
            num_periods: 0,
            pattern,
        }
    }

    pub fn set_callback(&mut self, cb: DmaCallback) {
        self.callback = Some(cb);
    }
}

// ---------------------------------------------------------------------------
// DMA fence
// ---------------------------------------------------------------------------

static FENCE_SEQNO: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FenceState {
    Unsignaled,
    Signaled,
    Error,
}

pub struct DmaFence {
    pub seqno: u64,
    pub context: u64,
    pub state: FenceState,
    pub timestamp: u64,
}

impl DmaFence {
    pub fn new(context: u64) -> Self {
        Self {
            seqno: FENCE_SEQNO.fetch_add(1, Ordering::Relaxed),
            context,
            state: FenceState::Unsignaled,
            timestamp: 0,
        }
    }

    pub fn signal(&mut self) {
        self.state = FenceState::Signaled;
        self.timestamp = read_tsc();
    }

    pub fn signal_error(&mut self) {
        self.state = FenceState::Error;
        self.timestamp = read_tsc();
    }

    pub fn is_signaled(&self) -> bool {
        self.state == FenceState::Signaled
    }

    pub fn wait(&self, timeout_us: u64) -> bool {
        let deadline = read_tsc() + timeout_us * 1000;
        loop {
            if self.state != FenceState::Unsignaled {
                return self.state == FenceState::Signaled;
            }
            if read_tsc() >= deadline {
                return false;
            }
            core::hint::spin_loop();
        }
    }
}

pub struct TimelineFence {
    pub context: u64,
    pub current_value: AtomicU64,
    pub fences: Vec<(u64, DmaFence)>,
}

impl TimelineFence {
    pub fn new(context: u64) -> Self {
        Self {
            context,
            current_value: AtomicU64::new(0),
            fences: Vec::new(),
        }
    }

    pub fn create_point(&mut self, value: u64) -> &mut DmaFence {
        let fence = DmaFence::new(self.context);
        self.fences.push((value, fence));
        &mut self.fences.last_mut().unwrap().1
    }

    pub fn signal(&mut self, value: u64) {
        self.current_value.store(value, Ordering::Release);
        for (v, fence) in &mut self.fences {
            if *v <= value && fence.state == FenceState::Unsignaled {
                fence.signal();
            }
        }
    }
}

pub fn fence_merge(a: &DmaFence, b: &DmaFence) -> DmaFence {
    let mut merged = DmaFence::new(a.context);
    if a.is_signaled() && b.is_signaled() {
        merged.signal();
    }
    merged
}

fn read_tsc() -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::x86_64::_rdtsc()
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        0
    }
}

// ---------------------------------------------------------------------------
// IOMMU integration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct IommuTranslation {
    iova: u64,
    phys: u64,
    size: u64,
}

struct IommuDomain {
    id: u32,
    name: String,
    bypass: bool,
    translations: Vec<IommuTranslation>,
    devices: Vec<u32>,
}

impl IommuDomain {
    fn new(id: u32, name: &str) -> Self {
        Self {
            id,
            name: String::from(name),
            bypass: false,
            translations: Vec::new(),
            devices: Vec::new(),
        }
    }

    fn map(&mut self, iova: u64, phys: u64, size: u64) {
        self.translations
            .push(IommuTranslation { iova, phys, size });
    }

    fn unmap(&mut self, iova: u64, size: u64) {
        self.translations
            .retain(|t| !(t.iova == iova && t.size == size));
    }

    fn attach_device(&mut self, dev_id: u32) {
        if !self.devices.contains(&dev_id) {
            self.devices.push(dev_id);
        }
    }

    fn detach_device(&mut self, dev_id: u32) {
        self.devices.retain(|d| *d != dev_id);
    }

    fn set_bypass(&mut self, bypass: bool) {
        self.bypass = bypass;
    }

    fn translate(&self, iova: u64) -> Option<u64> {
        if self.bypass {
            return Some(iova);
        }
        for t in &self.translations {
            if iova >= t.iova && iova < t.iova + t.size {
                return Some(t.phys + (iova - t.iova));
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Bounce buffering
// ---------------------------------------------------------------------------

struct BounceBuffer {
    cpu_addr: u64,
    dma_addr: u64,
    original_addr: u64,
    size: usize,
    direction: DmaDirection,
}

struct BouncePool {
    base: u64,
    size: usize,
    next_offset: usize,
    active_buffers: Vec<BounceBuffer>,
    limit_32bit: bool,
}

impl BouncePool {
    fn new(base: u64, size: usize, limit_32bit: bool) -> Self {
        Self {
            base,
            size,
            next_offset: 0,
            active_buffers: Vec::new(),
            limit_32bit,
        }
    }

    fn needs_bounce(&self, addr: u64) -> bool {
        if self.limit_32bit && addr >= 0x1_0000_0000 {
            return true;
        }
        false
    }

    fn alloc_bounce(&mut self, original: u64, size: usize, dir: DmaDirection) -> Option<u64> {
        let aligned = (self.next_offset + 63) & !63;
        if aligned + size > self.size {
            return None;
        }
        let bounce_addr = self.base + aligned as u64;
        self.next_offset = aligned + size;
        self.active_buffers.push(BounceBuffer {
            cpu_addr: bounce_addr,
            dma_addr: bounce_addr,
            original_addr: original,
            size,
            direction: dir,
        });
        Some(bounce_addr)
    }

    fn free_bounce(&mut self, dma_addr: u64) {
        self.active_buffers.retain(|b| b.dma_addr != dma_addr);
    }
}

// ---------------------------------------------------------------------------
// DMA debug
// ---------------------------------------------------------------------------

struct DmaDebug {
    enabled: AtomicBool,
    map_count: AtomicU64,
    unmap_count: AtomicU64,
    alloc_count: AtomicU64,
    free_count: AtomicU64,
    overrun_detect: AtomicBool,
}

impl DmaDebug {
    const fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            map_count: AtomicU64::new(0),
            unmap_count: AtomicU64::new(0),
            alloc_count: AtomicU64::new(0),
            free_count: AtomicU64::new(0),
            overrun_detect: AtomicBool::new(false),
        }
    }

    fn enable(&self) {
        self.enabled.store(true, Ordering::SeqCst);
    }

    fn disable(&self) {
        self.enabled.store(false, Ordering::SeqCst);
    }

    fn check_leaks(&self) -> i64 {
        let maps = self.map_count.load(Ordering::Relaxed) as i64;
        let unmaps = self.unmap_count.load(Ordering::Relaxed) as i64;
        maps - unmaps
    }

    fn stats(&self) -> DmaDebugStats {
        DmaDebugStats {
            map_count: self.map_count.load(Ordering::Relaxed),
            unmap_count: self.unmap_count.load(Ordering::Relaxed),
            alloc_count: self.alloc_count.load(Ordering::Relaxed),
            free_count: self.free_count.load(Ordering::Relaxed),
            active_mappings: self.check_leaks(),
        }
    }
}

pub struct DmaDebugStats {
    pub map_count: u64,
    pub unmap_count: u64,
    pub alloc_count: u64,
    pub free_count: u64,
    pub active_mappings: i64,
}

// ---------------------------------------------------------------------------
// Virtual DMA channels
// ---------------------------------------------------------------------------

struct VirtualChannel {
    id: u32,
    name: String,
    phys_channel: Option<u32>,
    pending_descriptors: Vec<DmaDescriptor>,
    active_descriptor: Option<DmaDescriptor>,
    state: ChannelState,
}

impl VirtualChannel {
    fn new(id: u32, name: &str) -> Self {
        Self {
            id,
            name: String::from(name),
            phys_channel: None,
            pending_descriptors: Vec::new(),
            active_descriptor: None,
            state: ChannelState::Idle,
        }
    }

    fn submit(&mut self, desc: DmaDescriptor) {
        self.pending_descriptors.push(desc);
    }

    fn issue_pending(&mut self) -> Option<&DmaDescriptor> {
        if self.active_descriptor.is_some() {
            return self.active_descriptor.as_ref();
        }
        if let Some(mut desc) = self.pending_descriptors.pop() {
            desc.status = DescriptorStatus::InProgress;
            self.active_descriptor = Some(desc);
            self.state = ChannelState::InProgress;
        }
        self.active_descriptor.as_ref()
    }

    fn complete_active(&mut self) -> Option<DmaDescriptor> {
        if let Some(mut desc) = self.active_descriptor.take() {
            desc.status = DescriptorStatus::Completed;
            self.state = if self.pending_descriptors.is_empty() {
                ChannelState::Idle
            } else {
                ChannelState::InProgress
            };
            Some(desc)
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// DMA request routing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct DmaRequestLine {
    line_id: u32,
    peripheral_name: String,
    channel_id: u32,
    priority: u8,
}

struct DmaRequestRouter {
    lines: Vec<DmaRequestLine>,
}

impl DmaRequestRouter {
    fn new() -> Self {
        Self { lines: Vec::new() }
    }

    fn add_route(&mut self, line_id: u32, peripheral: &str, channel_id: u32, priority: u8) {
        self.lines.push(DmaRequestLine {
            line_id,
            peripheral_name: String::from(peripheral),
            channel_id,
            priority,
        });
    }

    fn get_channel_for_peripheral(&self, name: &str) -> Option<u32> {
        self.lines
            .iter()
            .find(|l| l.peripheral_name == name)
            .map(|l| l.channel_id)
    }

    fn get_channel_for_line(&self, line_id: u32) -> Option<u32> {
        self.lines
            .iter()
            .find(|l| l.line_id == line_id)
            .map(|l| l.channel_id)
    }

    fn remove_route(&mut self, line_id: u32) {
        self.lines.retain(|l| l.line_id != line_id);
    }
}

// ---------------------------------------------------------------------------
// DMA engine (global state)
// ---------------------------------------------------------------------------

struct DmaEngine {
    channels: Vec<DmaChannel>,
    virtual_channels: Vec<VirtualChannel>,
    active_mappings: Vec<DmaMapping>,
    iommu_domains: Vec<IommuDomain>,
    bounce_pool: BouncePool,
    request_router: DmaRequestRouter,
    debug: DmaDebug,
    next_channel_id: u32,
    next_vchan_id: u32,
    next_mapping_id_val: u64,
    next_iommu_domain_id: u32,
    coherent_next_addr: u64,
    /// Freed coherent ranges available for reuse before bumping
    /// `coherent_next_addr` (BUG-28: a pure bump pointer leaked the address
    /// space on every alloc/free cycle). rangemap merges adjacent frees.
    coherent_free: rangemap::RangeSet<u64>,
    pool_next_addr: u64,
    fences: Vec<DmaFence>,
    timelines: Vec<TimelineFence>,
}

impl DmaEngine {
    fn new() -> Self {
        Self {
            channels: Vec::new(),
            virtual_channels: Vec::new(),
            active_mappings: Vec::new(),
            iommu_domains: Vec::new(),
            bounce_pool: BouncePool::new(0x0080_0000, 0x0010_0000, true),
            request_router: DmaRequestRouter::new(),
            debug: DmaDebug::new(),
            next_channel_id: 1,
            next_vchan_id: 1,
            next_mapping_id_val: 1,
            next_iommu_domain_id: 1,
            coherent_next_addr: 0x2000_0000,
            coherent_free: rangemap::RangeSet::new(),
            pool_next_addr: 0x3000_0000,
            fences: Vec::new(),
            timelines: Vec::new(),
        }
    }

    fn next_mapping_id(&mut self) -> u64 {
        let id = self.next_mapping_id_val;
        self.next_mapping_id_val += 1;
        id
    }

    fn alloc_channel(&mut self, name: &str) -> u32 {
        let id = self.next_channel_id;
        self.next_channel_id += 1;
        self.channels.push(DmaChannel::new(id, name));
        id
    }

    fn release_channel(&mut self, id: u32) {
        self.channels.retain(|c| c.id != id);
    }

    fn get_channel(&self, id: u32) -> Option<&DmaChannel> {
        self.channels.iter().find(|c| c.id == id)
    }

    fn get_channel_mut(&mut self, id: u32) -> Option<&mut DmaChannel> {
        self.channels.iter_mut().find(|c| c.id == id)
    }

    fn configure_channel(&mut self, id: u32, config: DmaChannelConfig) -> Result<(), &'static str> {
        let ch = self.get_channel_mut(id).ok_or("channel not found")?;
        ch.config = config;
        Ok(())
    }

    fn prep_memcpy(&mut self, chan_id: u32, src: u64, dst: u64, len: usize) -> Option<DmaCookie> {
        let ch = self.get_channel_mut(chan_id)?;
        let cookie = ch.next_cookie();
        let desc = DmaDescriptor::new_memcpy(cookie, src, dst, len);
        if let Some(vchan) = self
            .virtual_channels
            .iter_mut()
            .find(|v| v.phys_channel == Some(chan_id))
        {
            vchan.submit(desc);
        }
        Some(cookie)
    }

    fn prep_slave(
        &mut self,
        chan_id: u32,
        dev_addr: u64,
        mem_addr: u64,
        len: usize,
        dir: DmaDirection,
    ) -> Option<DmaCookie> {
        let ch = self.get_channel_mut(chan_id)?;
        let cookie = ch.next_cookie();
        let _desc = DmaDescriptor::new_slave(cookie, dev_addr, mem_addr, len, dir);
        Some(cookie)
    }

    fn prep_cyclic(
        &mut self,
        chan_id: u32,
        buf_addr: u64,
        buf_len: usize,
        period_len: usize,
        dir: DmaDirection,
        dev_addr: u64,
    ) -> Option<DmaCookie> {
        let ch = self.get_channel_mut(chan_id)?;
        let cookie = ch.next_cookie();
        let _desc = DmaDescriptor::new_cyclic(cookie, buf_addr, buf_len, period_len, dir, dev_addr);
        Some(cookie)
    }

    fn prep_memset(
        &mut self,
        chan_id: u32,
        dst: u64,
        pattern: u8,
        len: usize,
    ) -> Option<DmaCookie> {
        let ch = self.get_channel_mut(chan_id)?;
        let cookie = ch.next_cookie();
        let _desc = DmaDescriptor::new_memset(cookie, dst, pattern, len);
        Some(cookie)
    }

    fn alloc_virtual_channel(&mut self, name: &str) -> u32 {
        let id = self.next_vchan_id;
        self.next_vchan_id += 1;
        self.virtual_channels.push(VirtualChannel::new(id, name));
        id
    }

    fn bind_vchan_to_phys(&mut self, vchan_id: u32, phys_id: u32) -> Result<(), &'static str> {
        let vchan = self
            .virtual_channels
            .iter_mut()
            .find(|v| v.id == vchan_id)
            .ok_or("virtual channel not found")?;
        if self.channels.iter().any(|c| c.id == phys_id) {
            vchan.phys_channel = Some(phys_id);
            Ok(())
        } else {
            Err("physical channel not found")
        }
    }

    fn create_iommu_domain(&mut self, name: &str) -> u32 {
        let id = self.next_iommu_domain_id;
        self.next_iommu_domain_id += 1;
        self.iommu_domains.push(IommuDomain::new(id, name));
        id
    }

    fn iommu_map(
        &mut self,
        domain_id: u32,
        iova: u64,
        phys: u64,
        size: u64,
    ) -> Result<(), &'static str> {
        let domain = self
            .iommu_domains
            .iter_mut()
            .find(|d| d.id == domain_id)
            .ok_or("IOMMU domain not found")?;
        domain.map(iova, phys, size);
        Ok(())
    }

    fn iommu_unmap(&mut self, domain_id: u32, iova: u64, size: u64) -> Result<(), &'static str> {
        let domain = self
            .iommu_domains
            .iter_mut()
            .find(|d| d.id == domain_id)
            .ok_or("IOMMU domain not found")?;
        domain.unmap(iova, size);
        Ok(())
    }

    fn iommu_attach_device(&mut self, domain_id: u32, dev_id: u32) -> Result<(), &'static str> {
        let domain = self
            .iommu_domains
            .iter_mut()
            .find(|d| d.id == domain_id)
            .ok_or("IOMMU domain not found")?;
        domain.attach_device(dev_id);
        Ok(())
    }

    fn iommu_set_bypass(&mut self, domain_id: u32, bypass: bool) -> Result<(), &'static str> {
        let domain = self
            .iommu_domains
            .iter_mut()
            .find(|d| d.id == domain_id)
            .ok_or("IOMMU domain not found")?;
        domain.set_bypass(bypass);
        Ok(())
    }

    fn add_dma_route(&mut self, line_id: u32, peripheral: &str, channel_id: u32, priority: u8) {
        self.request_router
            .add_route(line_id, peripheral, channel_id, priority);
    }

    fn get_channel_for_peripheral(&self, name: &str) -> Option<u32> {
        self.request_router.get_channel_for_peripheral(name)
    }

    fn bounce_map(&mut self, addr: u64, size: usize, dir: DmaDirection) -> Option<u64> {
        if self.bounce_pool.needs_bounce(addr) {
            self.bounce_pool.alloc_bounce(addr, size, dir)
        } else {
            Some(addr)
        }
    }

    fn bounce_unmap(&mut self, dma_addr: u64) {
        self.bounce_pool.free_bounce(dma_addr);
    }
}

// ---------------------------------------------------------------------------
// Public channel API
// ---------------------------------------------------------------------------

pub fn dma_alloc_channel(name: &str) -> Option<u32> {
    let mut engine = DMA_ENGINE.lock();
    engine.as_mut().map(|e| e.alloc_channel(name))
}

pub fn dma_release_channel(id: u32) {
    let mut engine = DMA_ENGINE.lock();
    if let Some(e) = engine.as_mut() {
        e.release_channel(id);
    }
}

pub fn dma_configure_channel(id: u32, config: DmaChannelConfig) -> Result<(), &'static str> {
    let mut engine = DMA_ENGINE.lock();
    let e = engine.as_mut().ok_or("DMA engine not initialized")?;
    e.configure_channel(id, config)
}

pub fn dma_prep_memcpy(chan_id: u32, src: u64, dst: u64, len: usize) -> Option<DmaCookie> {
    let mut engine = DMA_ENGINE.lock();
    engine.as_mut()?.prep_memcpy(chan_id, src, dst, len)
}

pub fn dma_prep_slave(
    chan_id: u32,
    dev_addr: u64,
    mem_addr: u64,
    len: usize,
    dir: DmaDirection,
) -> Option<DmaCookie> {
    let mut engine = DMA_ENGINE.lock();
    engine
        .as_mut()?
        .prep_slave(chan_id, dev_addr, mem_addr, len, dir)
}

pub fn dma_prep_cyclic(
    chan_id: u32,
    buf_addr: u64,
    buf_len: usize,
    period_len: usize,
    dir: DmaDirection,
    dev_addr: u64,
) -> Option<DmaCookie> {
    let mut engine = DMA_ENGINE.lock();
    engine
        .as_mut()?
        .prep_cyclic(chan_id, buf_addr, buf_len, period_len, dir, dev_addr)
}

pub fn dma_prep_memset(chan_id: u32, dst: u64, pattern: u8, len: usize) -> Option<DmaCookie> {
    let mut engine = DMA_ENGINE.lock();
    engine.as_mut()?.prep_memset(chan_id, dst, pattern, len)
}

pub fn dma_debug_stats() -> Option<DmaDebugStats> {
    let engine = DMA_ENGINE.lock();
    engine.as_ref().map(|e| e.debug.stats())
}

pub fn dma_debug_enable() {
    let engine = DMA_ENGINE.lock();
    if let Some(e) = engine.as_ref() {
        e.debug.enable();
    }
}

pub fn dma_debug_check_leaks() -> i64 {
    let engine = DMA_ENGINE.lock();
    engine.as_ref().map(|e| e.debug.check_leaks()).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Global DMA_ENGINE
// ---------------------------------------------------------------------------

pub static DMA_ENGINE: Mutex<Option<DmaEngine>> = Mutex::new(None);

pub fn init() {
    let mut engine = DMA_ENGINE.lock();
    let mut e = DmaEngine::new();

    for i in 0..8 {
        let name = alloc::format!("dma-chan-{}", i);
        e.alloc_channel(&name);
    }

    for i in 0..16 {
        let name = alloc::format!("dma-vchan-{}", i);
        e.alloc_virtual_channel(&name);
    }

    let dom_id = e.create_iommu_domain("default");
    let _ = e.iommu_set_bypass(dom_id, true);

    e.add_dma_route(0, "spi0-tx", 1, 0);
    e.add_dma_route(1, "spi0-rx", 2, 0);
    e.add_dma_route(2, "i2s0-tx", 3, 1);
    e.add_dma_route(3, "i2s0-rx", 4, 1);
    e.add_dma_route(4, "uart0-tx", 5, 2);
    e.add_dma_route(5, "uart0-rx", 6, 2);

    *engine = Some(e);
}
