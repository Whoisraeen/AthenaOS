#![allow(dead_code)]

extern crate alloc;

use alloc::{collections::BTreeMap, format, string::String, vec::Vec};
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// Error type
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum DebugError {
    SymbolNotFound(String),
    ProbeAlreadyExists(u64),
    ProbeNotFound(u64),
    InvalidAddress(u64),
    CounterUnavailable,
    DumpFailed(String),
    InternalError(String),
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. Kernel Ring Buffer Logger
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum LogLevel {
    Emergency = 0,
    Alert = 1,
    Critical = 2,
    Error = 3,
    Warning = 4,
    Notice = 5,
    Info = 6,
    Debug = 7,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFacility {
    Kern,
    User,
    Daemon,
    Auth,
    Syslog,
    Lpr,
    News,
    Cron,
    Local0,
    Local1,
    Local2,
    Local3,
    Local4,
    Local5,
    Local6,
    Local7,
}

pub struct LogFilter {
    subsystem: Option<String>,
    min_level: LogLevel,
    enabled: bool,
}

pub struct LogEntry {
    sequence: u64,
    timestamp_ns: u64,
    level: LogLevel,
    facility: LogFacility,
    subsystem: String,
    message: String,
    cpu: u32,
    pid: Option<u64>,
    dict: Vec<(String, String)>,
}

pub struct LogStats {
    total_messages: u64,
    dropped: u64,
    by_level: [u64; 8],
}

pub struct KernelLog {
    buffer: Vec<LogEntry>,
    capacity: usize,
    head: usize,
    tail: usize,
    sequence: u64,
    filters: Vec<LogFilter>,
    console_level: LogLevel,
    persistent_path: Option<String>,
}

impl KernelLog {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(capacity),
            capacity,
            head: 0,
            tail: 0,
            sequence: 0,
            filters: Vec::new(),
            console_level: LogLevel::Warning,
            persistent_path: None,
        }
    }

    pub fn log(&mut self, level: LogLevel, subsystem: &str, message: &str) {
        self.log_with_dict(level, subsystem, message, &[]);
    }

    pub fn log_with_dict(
        &mut self,
        level: LogLevel,
        subsystem: &str,
        message: &str,
        dict: &[(&str, &str)],
    ) {
        for filter in &self.filters {
            if !filter.enabled {
                continue;
            }
            if let Some(ref sub) = filter.subsystem {
                if sub == subsystem && level > filter.min_level {
                    return;
                }
            } else if level > filter.min_level {
                return;
            }
        }

        let entry = LogEntry {
            sequence: self.sequence,
            timestamp_ns: Self::read_timestamp(),
            level,
            facility: LogFacility::Kern,
            subsystem: String::from(subsystem),
            message: String::from(message),
            cpu: Self::current_cpu(),
            pid: Self::current_pid(),
            dict: dict
                .iter()
                .map(|(k, v)| (String::from(*k), String::from(*v)))
                .collect(),
        };

        self.sequence += 1;

        if self.buffer.len() < self.capacity {
            self.buffer.push(entry);
            self.tail = self.buffer.len();
        } else {
            let idx = self.head % self.capacity;
            self.buffer[idx] = entry;
            self.head = self.head.wrapping_add(1);
            self.tail = self.tail.wrapping_add(1);
        }
    }

    pub fn read(&self, from_seq: u64, count: usize) -> Vec<&LogEntry> {
        self.buffer
            .iter()
            .filter(|e| e.sequence >= from_seq)
            .take(count)
            .collect()
    }

    pub fn read_level(&self, level: LogLevel) -> Vec<&LogEntry> {
        self.buffer.iter().filter(|e| e.level == level).collect()
    }

    pub fn read_subsystem(&self, subsystem: &str) -> Vec<&LogEntry> {
        self.buffer
            .iter()
            .filter(|e| e.subsystem == subsystem)
            .collect()
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
        self.head = 0;
        self.tail = 0;
    }

    pub fn set_console_level(&mut self, level: LogLevel) {
        self.console_level = level;
    }

    pub fn add_filter(&mut self, filter: LogFilter) {
        self.filters.push(filter);
    }

    pub fn stats(&self) -> LogStats {
        let mut by_level = [0u64; 8];
        for entry in &self.buffer {
            by_level[entry.level as usize] += 1;
        }
        let dropped = if self.sequence as usize > self.capacity {
            self.sequence - self.capacity as u64
        } else {
            0
        };
        LogStats {
            total_messages: self.sequence,
            dropped,
            by_level,
        }
    }

    pub fn dump_to_buffer(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for entry in &self.buffer {
            let line = format!(
                "[{:>5}.{:09}] <{}> {}: {}\n",
                entry.timestamp_ns / 1_000_000_000,
                entry.timestamp_ns % 1_000_000_000,
                entry.level as u8,
                entry.subsystem,
                entry.message,
            );
            out.extend_from_slice(line.as_bytes());
            for (k, v) in &entry.dict {
                let dict_line = format!("  {}={}\n", k, v);
                out.extend_from_slice(dict_line.as_bytes());
            }
        }
        out
    }

    fn read_timestamp() -> u64 {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            core::arch::x86_64::_rdtsc()
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            0
        }
    }

    fn current_cpu() -> u32 {
        0
    }

    fn current_pid() -> Option<u64> {
        None
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Kprobes (Dynamic Tracing)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct CpuRegisters {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub rsp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,
    pub rflags: u64,
    pub cs: u64,
    pub ss: u64,
    pub ds: u64,
    pub es: u64,
    pub fs: u64,
    pub gs: u64,
    pub cr2: u64,
    pub cr3: u64,
}

impl CpuRegisters {
    pub fn zeroed() -> Self {
        Self {
            rax: 0,
            rbx: 0,
            rcx: 0,
            rdx: 0,
            rsi: 0,
            rdi: 0,
            rbp: 0,
            rsp: 0,
            r8: 0,
            r9: 0,
            r10: 0,
            r11: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rip: 0,
            rflags: 0,
            cs: 0,
            ss: 0,
            ds: 0,
            es: 0,
            fs: 0,
            gs: 0,
            cr2: 0,
            cr3: 0,
        }
    }
}

pub struct KprobeFlags {
    disabled: bool,
    gone: bool,
    on_func_entry: bool,
}

pub struct KprobeContext {
    pub regs: CpuRegisters,
    pub address: u64,
    pub symbol: Option<String>,
    pub cpu: u32,
    pub pid: u64,
    pub timestamp_ns: u64,
}

pub struct Kprobe {
    id: u64,
    address: u64,
    symbol: Option<String>,
    offset: u32,
    pre_handler: Option<fn(&KprobeContext)>,
    post_handler: Option<fn(&KprobeContext, u64)>,
    fault_handler: Option<fn(&KprobeContext) -> bool>,
    saved_opcode: [u8; 16],
    enabled: bool,
    hit_count: u64,
    miss_count: u64,
    flags: KprobeFlags,
}

pub struct KretprobeContext {
    entry_regs: CpuRegisters,
    return_value: u64,
    entry_timestamp: u64,
    return_timestamp: u64,
}

pub struct KretprobeInstance {
    task: u64,
    entry_rip: u64,
    return_address: u64,
    entry_timestamp: u64,
}

pub struct KretProbe {
    id: u64,
    entry_address: u64,
    handler: fn(&KretprobeContext),
    max_active: u32,
    instances: Vec<KretprobeInstance>,
    hit_count: u64,
}

pub struct KprobeManager {
    probes: BTreeMap<u64, Kprobe>,
    kretprobes: BTreeMap<u64, KretProbe>,
    symbol_table: BTreeMap<String, u64>,
    breakpoint_handler_installed: bool,
    next_id: u64,
}

impl KprobeManager {
    pub fn new() -> Self {
        Self {
            probes: BTreeMap::new(),
            kretprobes: BTreeMap::new(),
            symbol_table: BTreeMap::new(),
            breakpoint_handler_installed: false,
            next_id: 1,
        }
    }

    pub fn register_kprobe(
        &mut self,
        symbol: &str,
        offset: u32,
        pre: Option<fn(&KprobeContext)>,
        post: Option<fn(&KprobeContext, u64)>,
    ) -> Result<u64, DebugError> {
        let base_addr = self
            .resolve_symbol(symbol)
            .ok_or_else(|| DebugError::SymbolNotFound(String::from(symbol)))?;

        let address = base_addr + offset as u64;

        if self.probes.contains_key(&address) {
            return Err(DebugError::ProbeAlreadyExists(address));
        }

        let id = self.next_id;
        self.next_id += 1;

        let probe = Kprobe {
            id,
            address,
            symbol: Some(String::from(symbol)),
            offset,
            pre_handler: pre,
            post_handler: post,
            fault_handler: None,
            saved_opcode: [0u8; 16],
            enabled: true,
            hit_count: 0,
            miss_count: 0,
            flags: KprobeFlags {
                disabled: false,
                gone: false,
                on_func_entry: offset == 0,
            },
        };

        self.arm_probe(address)?;
        self.probes.insert(address, probe);

        if !self.breakpoint_handler_installed {
            self.breakpoint_handler_installed = true;
        }

        Ok(id)
    }

    pub fn unregister_kprobe(&mut self, id: u64) -> Result<(), DebugError> {
        let addr = self
            .probes
            .iter()
            .find(|(_, p)| p.id == id)
            .map(|(a, _)| *a)
            .ok_or(DebugError::ProbeNotFound(id))?;

        self.disarm_probe(addr);
        self.probes.remove(&addr);
        Ok(())
    }

    pub fn register_kretprobe(
        &mut self,
        symbol: &str,
        handler: fn(&KretprobeContext),
        max_active: u32,
    ) -> Result<u64, DebugError> {
        let address = self
            .resolve_symbol(symbol)
            .ok_or_else(|| DebugError::SymbolNotFound(String::from(symbol)))?;

        let id = self.next_id;
        self.next_id += 1;

        let kretprobe = KretProbe {
            id,
            entry_address: address,
            handler,
            max_active,
            instances: Vec::with_capacity(max_active as usize),
            hit_count: 0,
        };

        self.arm_probe(address)?;
        self.kretprobes.insert(address, kretprobe);
        Ok(id)
    }

    pub fn unregister_kretprobe(&mut self, id: u64) -> Result<(), DebugError> {
        let addr = self
            .kretprobes
            .iter()
            .find(|(_, p)| p.id == id)
            .map(|(a, _)| *a)
            .ok_or(DebugError::ProbeNotFound(id))?;

        self.disarm_probe(addr);
        self.kretprobes.remove(&addr);
        Ok(())
    }

    pub fn enable_probe(&mut self, id: u64) {
        for probe in self.probes.values_mut() {
            if probe.id == id {
                probe.enabled = true;
                probe.flags.disabled = false;
                return;
            }
        }
    }

    pub fn disable_probe(&mut self, id: u64) {
        for probe in self.probes.values_mut() {
            if probe.id == id {
                probe.enabled = false;
                probe.flags.disabled = true;
                return;
            }
        }
    }

    pub fn handle_breakpoint(&mut self, regs: &CpuRegisters) -> bool {
        let addr = regs.rip.wrapping_sub(1);

        if let Some(probe) = self.probes.get_mut(&addr) {
            if !probe.enabled {
                probe.miss_count += 1;
                return true;
            }

            probe.hit_count += 1;

            let ctx = KprobeContext {
                regs: *regs,
                address: addr,
                symbol: probe.symbol.clone(),
                cpu: 0,
                pid: 0,
                timestamp_ns: KernelLog::read_timestamp(),
            };

            if let Some(pre) = probe.pre_handler {
                pre(&ctx);
            }

            if let Some(post) = probe.post_handler {
                post(&ctx, 0);
            }

            return true;
        }

        if let Some(krp) = self.kretprobes.get_mut(&addr) {
            krp.hit_count += 1;

            if krp.instances.len() < krp.max_active as usize {
                let instance = KretprobeInstance {
                    task: 0,
                    entry_rip: addr,
                    return_address: regs.rsp,
                    entry_timestamp: KernelLog::read_timestamp(),
                };
                krp.instances.push(instance);
            }

            return true;
        }

        false
    }

    pub fn list_probes(&self) -> Vec<&Kprobe> {
        self.probes.values().collect()
    }

    fn resolve_symbol(&self, name: &str) -> Option<u64> {
        self.symbol_table.get(name).copied()
    }

    fn arm_probe(&mut self, addr: u64) -> Result<(), DebugError> {
        if addr == 0 || addr > 0xFFFF_FFFF_FFFF_0000 {
            return Err(DebugError::InvalidAddress(addr));
        }
        // In production, this writes INT3 (0xCC) at the target address
        // after saving the original opcode. The saved opcode is used to
        // single-step past the original instruction after probe handlers run.
        if let Some(probe) = self.probes.get_mut(&addr) {
            unsafe {
                let ptr = addr as *const u8;
                for i in 0..16 {
                    probe.saved_opcode[i] = core::ptr::read_volatile(ptr.add(i));
                }
            }
        }
        Ok(())
    }

    fn disarm_probe(&mut self, addr: u64) {
        if let Some(probe) = self.probes.get(&addr) {
            unsafe {
                let ptr = addr as *mut u8;
                core::ptr::write_volatile(ptr, probe.saved_opcode[0]);
            }
        }
    }

    pub fn add_symbol(&mut self, name: &str, addr: u64) {
        self.symbol_table.insert(String::from(name), addr);
    }

    pub fn probe_stats(&self, id: u64) -> Option<(u64, u64)> {
        self.probes
            .values()
            .find(|p| p.id == id)
            .map(|p| (p.hit_count, p.miss_count))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. Ftrace (Function Tracer)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TracerType {
    Nop,
    Function,
    FunctionGraph,
    IrqsOff,
    Preemptoff,
    Wakeup,
    SchedSwitch,
    Hwlat,
}

pub struct TracedFunction {
    address: u64,
    name: String,
    call_count: u64,
    total_time_ns: u64,
    max_time_ns: u64,
    min_time_ns: u64,
}

#[derive(Clone)]
pub enum TraceEntryType {
    FunctionEntry {
        addr: u64,
        parent_addr: u64,
    },
    FunctionReturn {
        addr: u64,
        retval: u64,
        duration_ns: u64,
    },
    SchedSwitch {
        prev_pid: u64,
        prev_prio: i32,
        prev_state: u8,
        next_pid: u64,
        next_prio: i32,
    },
    SchedWakeup {
        pid: u64,
        prio: i32,
        target_cpu: u32,
    },
    IrqEntry {
        irq: u32,
        name: String,
    },
    IrqExit {
        irq: u32,
        handled: bool,
    },
    SoftirqEntry {
        vec: u32,
    },
    SoftirqExit {
        vec: u32,
    },
    Print {
        message: String,
    },
    UserStack {
        frames: Vec<u64>,
    },
    KernelStack {
        frames: Vec<u64>,
    },
}

pub struct TraceEntry {
    timestamp_ns: u64,
    cpu: u32,
    pid: u64,
    comm: String,
    entry_type: TraceEntryType,
}

pub struct TraceBuffer {
    entries: Vec<TraceEntry>,
    capacity: usize,
    head: usize,
    overrun: u64,
    cpu: u32,
}

impl TraceBuffer {
    pub fn new(cpu: u32, capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
            capacity,
            head: 0,
            overrun: 0,
            cpu,
        }
    }

    pub fn push(&mut self, entry: TraceEntry) {
        if self.entries.len() < self.capacity {
            self.entries.push(entry);
        } else {
            let idx = self.head % self.capacity;
            self.entries[idx] = entry;
            self.head += 1;
            self.overrun += 1;
        }
    }

    pub fn entries(&self) -> &[TraceEntry] {
        &self.entries
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.head = 0;
        self.overrun = 0;
    }
}

pub struct TraceRingBuffer {
    per_cpu: Vec<TraceBuffer>,
    global_clock: u64,
}

impl TraceRingBuffer {
    pub fn new(num_cpus: u32, per_cpu_capacity: usize) -> Self {
        let mut per_cpu = Vec::with_capacity(num_cpus as usize);
        for cpu in 0..num_cpus {
            per_cpu.push(TraceBuffer::new(cpu, per_cpu_capacity));
        }
        Self {
            per_cpu,
            global_clock: 0,
        }
    }
}

pub struct FtraceStats {
    entries: u64,
    overrun: u64,
    per_cpu_entries: Vec<(u32, u64)>,
}

pub struct FtraceManager {
    enabled: bool,
    traced_functions: BTreeMap<u64, TracedFunction>,
    ring_buffer: TraceRingBuffer,
    current_tracer: TracerType,
    filters: Vec<String>,
    notrace: Vec<String>,
    max_graph_depth: u32,
    function_count: u64,
}

impl FtraceManager {
    pub fn new(num_cpus: u32, buffer_size: usize) -> Self {
        Self {
            enabled: false,
            traced_functions: BTreeMap::new(),
            ring_buffer: TraceRingBuffer::new(num_cpus, buffer_size),
            current_tracer: TracerType::Nop,
            filters: Vec::new(),
            notrace: Vec::new(),
            max_graph_depth: 32,
            function_count: 0,
        }
    }

    pub fn enable(&mut self) {
        self.enabled = true;
    }

    pub fn disable(&mut self) {
        self.enabled = false;
    }

    pub fn set_tracer(&mut self, tracer: TracerType) {
        self.current_tracer = tracer;
    }

    pub fn add_filter(&mut self, pattern: &str) {
        self.filters.push(String::from(pattern));
    }

    pub fn add_notrace(&mut self, pattern: &str) {
        self.notrace.push(String::from(pattern));
    }

    pub fn clear_filters(&mut self) {
        self.filters.clear();
        self.notrace.clear();
    }

    pub fn record_function_entry(&mut self, addr: u64, parent: u64, cpu: u32, pid: u64) {
        if !self.enabled {
            return;
        }

        if self.current_tracer != TracerType::Function
            && self.current_tracer != TracerType::FunctionGraph
        {
            return;
        }

        self.function_count += 1;

        if let Some(func) = self.traced_functions.get_mut(&addr) {
            func.call_count += 1;
        }

        let entry = TraceEntry {
            timestamp_ns: KernelLog::read_timestamp(),
            cpu,
            pid,
            comm: String::from("unknown"),
            entry_type: TraceEntryType::FunctionEntry {
                addr,
                parent_addr: parent,
            },
        };

        if let Some(buf) = self.ring_buffer.per_cpu.get_mut(cpu as usize) {
            buf.push(entry);
        }
    }

    pub fn record_function_return(
        &mut self,
        addr: u64,
        retval: u64,
        duration_ns: u64,
        cpu: u32,
        pid: u64,
    ) {
        if !self.enabled {
            return;
        }

        if let Some(func) = self.traced_functions.get_mut(&addr) {
            func.total_time_ns += duration_ns;
            if duration_ns > func.max_time_ns {
                func.max_time_ns = duration_ns;
            }
            if duration_ns < func.min_time_ns || func.min_time_ns == 0 {
                func.min_time_ns = duration_ns;
            }
        }

        let entry = TraceEntry {
            timestamp_ns: KernelLog::read_timestamp(),
            cpu,
            pid,
            comm: String::from("unknown"),
            entry_type: TraceEntryType::FunctionReturn {
                addr,
                retval,
                duration_ns,
            },
        };

        if let Some(buf) = self.ring_buffer.per_cpu.get_mut(cpu as usize) {
            buf.push(entry);
        }
    }

    pub fn record_event(&mut self, cpu: u32, pid: u64, event: TraceEntryType) {
        if !self.enabled {
            return;
        }

        let entry = TraceEntry {
            timestamp_ns: KernelLog::read_timestamp(),
            cpu,
            pid,
            comm: String::from("unknown"),
            entry_type: event,
        };

        if let Some(buf) = self.ring_buffer.per_cpu.get_mut(cpu as usize) {
            buf.push(entry);
        }
    }

    pub fn read_trace(&self, cpu: Option<u32>) -> Vec<&TraceEntry> {
        match cpu {
            Some(c) => {
                if let Some(buf) = self.ring_buffer.per_cpu.get(c as usize) {
                    buf.entries().iter().collect()
                } else {
                    Vec::new()
                }
            }
            None => self
                .ring_buffer
                .per_cpu
                .iter()
                .flat_map(|buf| buf.entries().iter())
                .collect(),
        }
    }

    pub fn clear_trace(&mut self) {
        for buf in &mut self.ring_buffer.per_cpu {
            buf.clear();
        }
    }

    pub fn stats(&self) -> FtraceStats {
        let mut entries = 0u64;
        let mut overrun = 0u64;
        let mut per_cpu_entries = Vec::new();

        for buf in &self.ring_buffer.per_cpu {
            let count = buf.entries.len() as u64;
            entries += count;
            overrun += buf.overrun;
            per_cpu_entries.push((buf.cpu, count));
        }

        FtraceStats {
            entries,
            overrun,
            per_cpu_entries,
        }
    }

    pub fn function_profile(&self) -> Vec<&TracedFunction> {
        self.traced_functions.values().collect()
    }

    pub fn register_function(&mut self, addr: u64, name: &str) {
        self.traced_functions.insert(
            addr,
            TracedFunction {
                address: addr,
                name: String::from(name),
                call_count: 0,
                total_time_ns: 0,
                max_time_ns: 0,
                min_time_ns: u64::MAX,
            },
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. Performance Counters
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HwCounterType {
    CpuCycles,
    Instructions,
    CacheMisses,
    CacheReferences,
    BranchMisses,
    BranchInstructions,
    BusCycles,
    StalledCyclesFrontend,
    StalledCyclesBackend,
    L1dCacheLoads,
    L1dCacheMisses,
    L1iCacheLoads,
    L1iCacheMisses,
    LlcLoads,
    LlcMisses,
    DtlbLoads,
    DtlbMisses,
    ItlbLoads,
    ItlbMisses,
    ContextSwitches,
    CpuMigrations,
    PageFaults,
    AlignmentFaults,
}

impl HwCounterType {
    fn event_select(&self) -> u64 {
        match self {
            Self::CpuCycles => 0x003C,
            Self::Instructions => 0x00C0,
            Self::CacheMisses => 0x412E,
            Self::CacheReferences => 0x4F2E,
            Self::BranchMisses => 0x00C5,
            Self::BranchInstructions => 0x00C4,
            Self::BusCycles => 0x013C,
            Self::StalledCyclesFrontend => 0x019C,
            Self::StalledCyclesBackend => 0x01A2,
            Self::L1dCacheLoads => 0x0143,
            Self::L1dCacheMisses => 0x0151,
            Self::L1iCacheLoads => 0x0380,
            Self::L1iCacheMisses => 0x0280,
            Self::LlcLoads => 0x4F2E,
            Self::LlcMisses => 0x412E,
            Self::DtlbLoads => 0x0108,
            Self::DtlbMisses => 0x0508,
            Self::ItlbLoads => 0x0185,
            Self::ItlbMisses => 0x0185,
            Self::ContextSwitches => 0,
            Self::CpuMigrations => 0,
            Self::PageFaults => 0,
            Self::AlignmentFaults => 0,
        }
    }
}

pub struct HwCounter {
    counter_type: HwCounterType,
    msr_select: u32,
    msr_count: u32,
    config: u64,
    count: u64,
    enabled: bool,
    overflow_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwCounterType {
    TaskClock,
    ContextSwitches,
    CpuMigrations,
    PageFaultsMinor,
    PageFaultsMajor,
    AlignmentFaults,
    EmulationFaults,
}

pub struct SwCounter {
    counter_type: SwCounterType,
    count: u64,
}

#[derive(Clone)]
pub enum PerfEventType {
    Sample {
        period: u64,
    },
    Mmap {
        addr: u64,
        len: u64,
        pgoff: u64,
        filename: String,
    },
    Comm {
        comm: String,
    },
    Exit {
        pid: u64,
    },
    Fork {
        ppid: u64,
    },
    ContextSwitch,
}

pub struct PerfEvent {
    timestamp: u64,
    cpu: u32,
    pid: u64,
    event_type: PerfEventType,
    ip: u64,
    callchain: Vec<u64>,
}

pub struct PerfCounterManager {
    hardware_counters: Vec<HwCounter>,
    software_counters: Vec<SwCounter>,
    events: Vec<PerfEvent>,
    sampling_period: u64,
    enabled: bool,
}

impl PerfCounterManager {
    pub fn new() -> Self {
        Self {
            hardware_counters: Vec::new(),
            software_counters: Vec::new(),
            events: Vec::new(),
            sampling_period: 0,
            enabled: false,
        }
    }

    pub fn open_counter(&mut self, counter_type: HwCounterType) -> Result<usize, DebugError> {
        if self.hardware_counters.len() >= 8 {
            return Err(DebugError::CounterUnavailable);
        }

        let index = self.hardware_counters.len();
        let msr_base_select: u32 = 0x186;
        let msr_base_count: u32 = 0x0C1;

        let counter = HwCounter {
            counter_type,
            msr_select: msr_base_select + index as u32,
            msr_count: msr_base_count + index as u32,
            config: counter_type.event_select(),
            count: 0,
            enabled: false,
            overflow_count: 0,
        };

        self.hardware_counters.push(counter);
        self.setup_pmc(index, counter_type.event_select());
        Ok(index)
    }

    pub fn close_counter(&mut self, index: usize) {
        if index < self.hardware_counters.len() {
            self.hardware_counters[index].enabled = false;
            self.hardware_counters[index].config = 0;
        }
    }

    pub fn read_counter(&self, index: usize) -> u64 {
        if index < self.hardware_counters.len() {
            self.read_pmc(index)
        } else {
            0
        }
    }

    pub fn reset_counter(&mut self, index: usize) {
        if index < self.hardware_counters.len() {
            self.hardware_counters[index].count = 0;
            self.hardware_counters[index].overflow_count = 0;
        }
    }

    pub fn enable_all(&mut self) {
        self.enabled = true;
        for counter in &mut self.hardware_counters {
            counter.enabled = true;
        }
    }

    pub fn disable_all(&mut self) {
        self.enabled = false;
        for counter in &mut self.hardware_counters {
            counter.enabled = false;
        }
    }

    pub fn read_all(&self) -> Vec<(HwCounterType, u64)> {
        self.hardware_counters
            .iter()
            .enumerate()
            .map(|(i, c)| (c.counter_type, self.read_pmc(i)))
            .collect()
    }

    pub fn start_sampling(&mut self, period: u64) {
        self.sampling_period = period;
        self.enabled = true;
    }

    pub fn stop_sampling(&mut self) {
        self.sampling_period = 0;
        self.enabled = false;
    }

    pub fn read_events(&self) -> &[PerfEvent] {
        &self.events
    }

    pub fn record_event(&mut self, cpu: u32, pid: u64, event_type: PerfEventType, ip: u64) {
        if !self.enabled {
            return;
        }
        let event = PerfEvent {
            timestamp: KernelLog::read_timestamp(),
            cpu,
            pid,
            event_type,
            ip,
            callchain: Vec::new(),
        };
        self.events.push(event);
    }

    pub fn record_event_with_callchain(
        &mut self,
        cpu: u32,
        pid: u64,
        event_type: PerfEventType,
        ip: u64,
        callchain: Vec<u64>,
    ) {
        if !self.enabled {
            return;
        }
        let event = PerfEvent {
            timestamp: KernelLog::read_timestamp(),
            cpu,
            pid,
            event_type,
            ip,
            callchain,
        };
        self.events.push(event);
    }

    pub fn clear_events(&mut self) {
        self.events.clear();
    }

    pub fn increment_sw_counter(&mut self, counter_type: SwCounterType) {
        for counter in &mut self.software_counters {
            if core::mem::discriminant(&counter.counter_type)
                == core::mem::discriminant(&counter_type)
            {
                counter.count += 1;
                return;
            }
        }
        self.software_counters.push(SwCounter {
            counter_type,
            count: 1,
        });
    }

    fn setup_pmc(&mut self, index: usize, config: u64) {
        if index < self.hardware_counters.len() {
            self.hardware_counters[index].config = config;
            self.hardware_counters[index].enabled = true;
            // In production: wrmsr(msr_select, config | EN | USR | OS)
        }
    }

    fn read_pmc(&self, index: usize) -> u64 {
        if index < self.hardware_counters.len() {
            // In production: rdmsr(msr_count) or rdpmc(index)
            self.hardware_counters[index].count
        } else {
            0
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. Crash Dump
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum CrashReason {
    Panic(String),
    Oops(String),
    BugOn(String),
    DoubleFault,
    NMI,
    MachineCheck,
    Watchdog,
    StackOverflow,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DumpType {
    Full,
    Filtered,
    Mini,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemRegType {
    Usable,
    Reserved,
    AcpiReclaim,
    AcpiNvs,
    Bad,
    Kernel,
    Modules,
    Crash,
}

pub struct MemoryRegion {
    start: u64,
    end: u64,
    region_type: MemRegType,
}

pub struct StackFrame {
    address: u64,
    symbol: Option<String>,
    offset: u32,
    module: Option<String>,
}

pub struct CpuCrashInfo {
    cpu_id: u32,
    registers: CpuRegisters,
    stack: Vec<u8>,
    current_task: Option<u64>,
}

pub struct CrashHeader {
    magic: [u8; 8],
    version: u32,
    timestamp: u64,
    reason: CrashReason,
    cpu: u32,
    registers: CpuRegisters,
    stack_dump: Vec<u8>,
    backtrace: Vec<StackFrame>,
    memory_map: Vec<MemoryRegion>,
    log_tail: Vec<u8>,
    num_cpus: u32,
    per_cpu_info: Vec<CpuCrashInfo>,
}

pub struct CrashDumper {
    dump_type: DumpType,
    dump_device: Option<String>,
    reserved_memory: u64,
    header: CrashHeader,
}

impl CrashDumper {
    pub fn new(dump_type: DumpType) -> Self {
        Self {
            dump_type,
            dump_device: None,
            reserved_memory: 0,
            header: CrashHeader {
                magic: *b"RAECRASH",
                version: 1,
                timestamp: 0,
                reason: CrashReason::Unknown,
                cpu: 0,
                registers: CpuRegisters::zeroed(),
                stack_dump: Vec::new(),
                backtrace: Vec::new(),
                memory_map: Vec::new(),
                log_tail: Vec::new(),
                num_cpus: 1,
                per_cpu_info: Vec::new(),
            },
        }
    }

    pub fn prepare(&mut self, reason: CrashReason, regs: &CpuRegisters) -> CrashHeader {
        let backtrace = self.unwind_stack(regs.rsp, regs.rbp, regs.rip);
        let per_cpu_info = self.collect_per_cpu_info();
        let log_tail = self.capture_log_tail(4096);

        let stack_dump = unsafe {
            let mut dump = Vec::with_capacity(4096);
            let base = regs.rsp as *const u8;
            for i in 0..4096 {
                dump.push(core::ptr::read_volatile(base.add(i)));
            }
            dump
        };

        CrashHeader {
            magic: *b"RAECRASH",
            version: 1,
            timestamp: KernelLog::read_timestamp(),
            reason,
            cpu: 0,
            registers: *regs,
            stack_dump,
            backtrace,
            memory_map: Vec::new(),
            log_tail,
            num_cpus: 1,
            per_cpu_info,
        }
    }

    pub fn write_dump(&self, header: &CrashHeader) -> Result<(), DebugError> {
        if self.dump_device.is_none() {
            return Err(DebugError::DumpFailed(String::from(
                "no dump device configured",
            )));
        }

        let mut buf = Vec::new();
        buf.extend_from_slice(&header.magic);
        buf.extend_from_slice(&header.version.to_le_bytes());
        buf.extend_from_slice(&header.timestamp.to_le_bytes());
        buf.extend_from_slice(&(header.cpu).to_le_bytes());
        buf.extend_from_slice(&header.registers.rip.to_le_bytes());
        buf.extend_from_slice(&header.registers.rsp.to_le_bytes());
        buf.extend_from_slice(&header.registers.rbp.to_le_bytes());

        let bt_count = header.backtrace.len() as u32;
        buf.extend_from_slice(&bt_count.to_le_bytes());
        for frame in &header.backtrace {
            buf.extend_from_slice(&frame.address.to_le_bytes());
            buf.extend_from_slice(&frame.offset.to_le_bytes());
        }

        let stack_len = header.stack_dump.len() as u32;
        buf.extend_from_slice(&stack_len.to_le_bytes());
        buf.extend_from_slice(&header.stack_dump);

        let log_len = header.log_tail.len() as u32;
        buf.extend_from_slice(&log_len.to_le_bytes());
        buf.extend_from_slice(&header.log_tail);

        // In production: write buf to dump_device via block I/O
        Ok(())
    }

    pub fn unwind_stack(&self, rsp: u64, rbp: u64, rip: u64) -> Vec<StackFrame> {
        let mut frames = Vec::new();

        frames.push(StackFrame {
            address: rip,
            symbol: self.symbolize_address(rip).map(|(s, _)| s),
            offset: self.symbolize_address(rip).map(|(_, o)| o).unwrap_or(0),
            module: None,
        });

        let mut current_rbp = rbp;
        for _ in 0..64 {
            if current_rbp == 0 || !Self::is_valid_stack_addr(current_rbp) {
                break;
            }

            let return_addr = unsafe {
                let ptr = (current_rbp + 8) as *const u64;
                core::ptr::read_volatile(ptr)
            };

            if return_addr == 0 {
                break;
            }

            let (sym, off) = self
                .symbolize_address(return_addr)
                .unwrap_or((String::from("<unknown>"), 0));

            frames.push(StackFrame {
                address: return_addr,
                symbol: Some(sym),
                offset: off,
                module: None,
            });

            current_rbp = unsafe {
                let ptr = current_rbp as *const u64;
                core::ptr::read_volatile(ptr)
            };
        }

        frames
    }

    pub fn collect_per_cpu_info(&self) -> Vec<CpuCrashInfo> {
        alloc::vec![CpuCrashInfo {
            cpu_id: 0,
            registers: CpuRegisters::zeroed(),
            stack: Vec::new(),
            current_task: None,
        }]
    }

    pub fn capture_log_tail(&self, max_bytes: usize) -> Vec<u8> {
        let guard = DEBUG.lock();
        if let Some(ref debug) = *guard {
            let full = debug.log.dump_to_buffer();
            if full.len() <= max_bytes {
                full
            } else {
                full[full.len() - max_bytes..].to_vec()
            }
        } else {
            Vec::new()
        }
    }

    fn symbolize_address(&self, addr: u64) -> Option<(String, u32)> {
        let guard = DEBUG.lock();
        if let Some(ref debug) = *guard {
            for (name, &sym_addr) in &debug.kprobes.symbol_table {
                if addr >= sym_addr && addr < sym_addr + 0x1000 {
                    return Some((name.clone(), (addr - sym_addr) as u32));
                }
            }
        }
        None
    }

    fn is_valid_stack_addr(addr: u64) -> bool {
        addr > 0x1000 && addr < 0xFFFF_FFFF_FFFF_0000 && addr % 8 == 0
    }

    pub fn set_dump_device(&mut self, device: &str) {
        self.dump_device = Some(String::from(device));
    }

    pub fn set_reserved_memory(&mut self, bytes: u64) {
        self.reserved_memory = bytes;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 6. Stack Unwinder
// ─────────────────────────────────────────────────────────────────────────────

pub struct UnwindFrame {
    rip: u64,
    rsp: u64,
    rbp: u64,
    symbol: Option<String>,
    offset: u32,
    is_signal_frame: bool,
}

pub struct StackUnwinder {
    frame_pointer_mode: bool,
    dwarf_mode: bool,
    orc_mode: bool,
    symbol_table: BTreeMap<u64, (String, u64)>,
}

impl StackUnwinder {
    pub fn new() -> Self {
        Self {
            frame_pointer_mode: true,
            dwarf_mode: false,
            orc_mode: false,
            symbol_table: BTreeMap::new(),
        }
    }

    pub fn unwind_from_regs(&self, regs: &CpuRegisters, max_frames: usize) -> Vec<UnwindFrame> {
        let mut frames = Vec::with_capacity(max_frames);

        frames.push(UnwindFrame {
            rip: regs.rip,
            rsp: regs.rsp,
            rbp: regs.rbp,
            symbol: self.lookup_symbol(regs.rip).map(|(s, _)| String::from(s)),
            offset: self.lookup_symbol(regs.rip).map(|(_, o)| o).unwrap_or(0),
            is_signal_frame: false,
        });

        if self.frame_pointer_mode {
            let more = self.unwind_from_rbp(regs.rbp, regs.rip, max_frames - 1);
            frames.extend(more);
        }

        frames
    }

    pub fn unwind_from_rbp(&self, rbp: u64, _rip: u64, max_frames: usize) -> Vec<UnwindFrame> {
        let mut frames = Vec::new();
        let mut current_rbp = rbp;

        for _ in 0..max_frames {
            if current_rbp == 0 || !self.is_kernel_text(current_rbp) && current_rbp < 0x1000 {
                break;
            }

            let (next_rbp, return_addr) = match self.read_frame_pair(current_rbp) {
                Some(pair) => pair,
                None => break,
            };

            if return_addr == 0 {
                break;
            }

            let (sym, off) = match self.lookup_symbol(return_addr) {
                Some((s, o)) => (Some(String::from(s)), o),
                None => (None, 0),
            };

            let is_signal = self.detect_signal_frame(current_rbp);

            frames.push(UnwindFrame {
                rip: return_addr,
                rsp: current_rbp + 16,
                rbp: next_rbp,
                symbol: sym,
                offset: off,
                is_signal_frame: is_signal,
            });

            if next_rbp <= current_rbp {
                break;
            }
            current_rbp = next_rbp;
        }

        frames
    }

    pub fn format_backtrace(&self, frames: &[UnwindFrame]) -> String {
        let mut output = String::from("Call Trace:\n");
        for (i, frame) in frames.iter().enumerate() {
            output.push_str(&self.format_frame(frame, i));
            output.push('\n');
        }
        output
    }

    pub fn format_frame(&self, frame: &UnwindFrame, index: usize) -> String {
        let signal_marker = if frame.is_signal_frame {
            " [signal]"
        } else {
            ""
        };
        match &frame.symbol {
            Some(sym) => format!(
                " #{:<3} [{:#018x}] {}+{:#x}{}",
                index, frame.rip, sym, frame.offset, signal_marker
            ),
            None => format!(
                " #{:<3} [{:#018x}] <unknown>{}",
                index, frame.rip, signal_marker
            ),
        }
    }

    pub fn add_symbol(&mut self, addr: u64, name: &str, size: u64) {
        self.symbol_table.insert(addr, (String::from(name), size));
    }

    fn lookup_symbol(&self, addr: u64) -> Option<(&str, u32)> {
        for (&sym_addr, (name, size)) in self.symbol_table.iter().rev() {
            if addr >= sym_addr && addr < sym_addr + size {
                return Some((name.as_str(), (addr - sym_addr) as u32));
            }
        }
        None
    }

    fn is_kernel_text(&self, addr: u64) -> bool {
        const KERNEL_TEXT_START: u64 = 0xFFFF_FFFF_8000_0000;
        const KERNEL_TEXT_END: u64 = 0xFFFF_FFFF_FFFF_FFFF;
        addr >= KERNEL_TEXT_START && addr <= KERNEL_TEXT_END
    }

    fn read_stack_u64(&self, addr: u64) -> Option<u64> {
        if addr == 0 || addr % 8 != 0 {
            return None;
        }
        if addr > 0xFFFF_FFFF_FFFF_FFF0 {
            return None;
        }
        // Kernel unwinder: only ever walk KERNEL-half frame pointers. Rejecting
        // the user half both keeps the panic backtrace honest (a user rbp is not
        // a kernel frame) and is SMAP-safe — a raw supervisor read of a user
        // page under CR4.SMAP would fault inside the panic path. Unwinding just
        // stops at the boundary, which is the correct terminus anyway.
        if addr < 0xFFFF_8000_0000_0000 {
            return None;
        }
        Some(unsafe { core::ptr::read_volatile(addr as *const u64) })
    }

    fn read_frame_pair(&self, rbp: u64) -> Option<(u64, u64)> {
        let next_rbp = self.read_stack_u64(rbp)?;
        let ret_addr = self.read_stack_u64(rbp + 8)?;
        Some((next_rbp, ret_addr))
    }

    fn detect_signal_frame(&self, _rbp: u64) -> bool {
        // Heuristic: check for sigreturn trampoline at return address
        false
    }

    pub fn set_mode(&mut self, fp: bool, dwarf: bool, orc: bool) {
        self.frame_pointer_mode = fp;
        self.dwarf_mode = dwarf;
        self.orc_mode = orc;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 7. Watchdog Timer
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogAction {
    Panic,
    Reboot,
    Log,
    Nothing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogAlertType {
    SoftLockup,
    HardLockup,
}

pub struct WatchdogAlert {
    cpu: u32,
    type_: WatchdogAlertType,
    duration_s: u64,
}

pub struct Watchdog {
    timeout_s: u32,
    last_pet: u64,
    enabled: bool,
    action: WatchdogAction,
    per_cpu_timestamp: Vec<u64>,
    softlockup_threshold_s: u32,
    hardlockup_threshold_s: u32,
}

impl Watchdog {
    pub fn new(num_cpus: u32, timeout_s: u32) -> Self {
        Self {
            timeout_s,
            last_pet: 0,
            enabled: false,
            action: WatchdogAction::Panic,
            per_cpu_timestamp: alloc::vec![0u64; num_cpus as usize],
            softlockup_threshold_s: timeout_s * 2,
            hardlockup_threshold_s: timeout_s * 4,
        }
    }

    pub fn pet(&mut self, cpu: u32) {
        let now = KernelLog::read_timestamp();
        if let Some(ts) = self.per_cpu_timestamp.get_mut(cpu as usize) {
            *ts = now;
        }
        self.last_pet = now;
    }

    pub fn check(&self, now: u64) -> Option<WatchdogAlert> {
        if !self.enabled {
            return None;
        }

        for (cpu, &last) in self.per_cpu_timestamp.iter().enumerate() {
            if last == 0 {
                continue;
            }

            let elapsed_ns = now.saturating_sub(last);
            let elapsed_s = elapsed_ns / 1_000_000_000;

            if elapsed_s >= self.hardlockup_threshold_s as u64 {
                return Some(WatchdogAlert {
                    cpu: cpu as u32,
                    type_: WatchdogAlertType::HardLockup,
                    duration_s: elapsed_s,
                });
            }

            if elapsed_s >= self.softlockup_threshold_s as u64 {
                return Some(WatchdogAlert {
                    cpu: cpu as u32,
                    type_: WatchdogAlertType::SoftLockup,
                    duration_s: elapsed_s,
                });
            }
        }

        None
    }

    pub fn enable(&mut self) {
        self.enabled = true;
        let now = KernelLog::read_timestamp();
        for ts in &mut self.per_cpu_timestamp {
            *ts = now;
        }
    }

    pub fn disable(&mut self) {
        self.enabled = false;
    }

    pub fn set_action(&mut self, action: WatchdogAction) {
        self.action = action;
    }

    pub fn set_timeout(&mut self, seconds: u32) {
        self.timeout_s = seconds;
        self.softlockup_threshold_s = seconds * 2;
        self.hardlockup_threshold_s = seconds * 4;
    }

    pub fn action(&self) -> WatchdogAction {
        self.action
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Debug Subsystem Aggregate
// ─────────────────────────────────────────────────────────────────────────────

pub struct DebugSubsystem {
    pub log: KernelLog,
    pub kprobes: KprobeManager,
    pub ftrace: FtraceManager,
    pub perf: PerfCounterManager,
    pub crash: CrashDumper,
    pub unwinder: StackUnwinder,
    pub watchdog: Watchdog,
}

pub static DEBUG: Mutex<Option<DebugSubsystem>> = Mutex::new(None);

pub fn init() {
    let mut guard = DEBUG.lock();
    *guard = Some(DebugSubsystem {
        log: KernelLog::new(8192),
        kprobes: KprobeManager::new(),
        ftrace: FtraceManager::new(4, 16384),
        perf: PerfCounterManager::new(),
        crash: CrashDumper::new(DumpType::Filtered),
        unwinder: StackUnwinder::new(),
        watchdog: Watchdog::new(4, 10),
    });

    if let Some(ref mut dbg) = *guard {
        dbg.log
            .log(LogLevel::Info, "debug", "debug subsystem initialized");
        dbg.watchdog.enable();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Macros
// ─────────────────────────────────────────────────────────────────────────────

#[macro_export]
macro_rules! klog {
    ($level:expr, $subsys:expr, $($arg:tt)*) => {{
        let msg = alloc::format!($($arg)*);
        let mut guard = $crate::debug::DEBUG.lock();
        if let Some(ref mut dbg) = *guard {
            dbg.log.log($level, $subsys, &msg);
        }
    }};
}

#[macro_export]
macro_rules! klog_err {
    ($subsys:expr, $($arg:tt)*) => {
        $crate::klog!($crate::debug::LogLevel::Error, $subsys, $($arg)*)
    };
}

#[macro_export]
macro_rules! klog_warn {
    ($subsys:expr, $($arg:tt)*) => {
        $crate::klog!($crate::debug::LogLevel::Warning, $subsys, $($arg)*)
    };
}

#[macro_export]
macro_rules! klog_info {
    ($subsys:expr, $($arg:tt)*) => {
        $crate::klog!($crate::debug::LogLevel::Info, $subsys, $($arg)*)
    };
}

#[macro_export]
macro_rules! klog_debug {
    ($subsys:expr, $($arg:tt)*) => {
        $crate::klog!($crate::debug::LogLevel::Debug, $subsys, $($arg)*)
    };
}
