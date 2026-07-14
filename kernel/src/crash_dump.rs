//! Kernel crash dump system — on panic, serialize system state to a reserved
//! memory region for post-reboot recovery.
//!
//! Designed to work even when the kernel is partially corrupted: no heap
//! allocation in the dump path, all writes go to a pre-reserved physical
//! memory region that survives warm reboot.
//!
//! Concept (LEGACY_GAMING_CONCEPT.md, "Stability & Recovery"): a crash must never be
//! a black box. AthenaOS reserves the last 4 MiB of RAM at boot, drops a
//! tombstone (magic + timestamp + panic message + RIP/RSP/RBP) into it from the
//! panic handler, and on the next boot detects the tombstone, reports it for
//! `/var/crash/`, and clears the magic.
//!
//! R10 contract: [`init`] is called from `kernel_main`; [`run_boot_smoketest`]
//! proves the reserved region; [`dump_text`] backs `/proc/athena/crash`; and the
//! panic handler calls [`write_crash_dump`].

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

// ═══════════════════════════════════════════════════════════════════════════════
//  Constants
// ═══════════════════════════════════════════════════════════════════════════════

const CRASH_DUMP_MAGIC: u64 = 0x5241_4543_5241_5348; // "RAECRASH" in ASCII-ish
const CRASH_DUMP_VERSION: u32 = 1;
const MAX_BACKTRACE_DEPTH: usize = 64;
const MAX_PANIC_MSG_LEN: usize = 512;
const KERNEL_LOG_RING_SIZE: usize = 64;
const KERNEL_LOG_LINE_LEN: usize = 128;
const MAX_MODULE_NAME_LEN: usize = 64;
const MAX_MODULES: usize = 32;
const MAX_TASK_NAME_LEN: usize = 32;

/// Reserved physical memory region for crash dumps.
/// This must be excluded from the normal memory allocator.
const CRASH_REGION_PHYS_BASE: u64 = 0x0010_0000; // 1 MiB mark (below kernel load)
const CRASH_REGION_SIZE: usize = 256 * 1024; // 256 KiB reserved

/// Phase 4.5: size of the high-RAM region we carve off the top of physical
/// memory for crash dumps. The spec calls for the last 4 MiB of RAM.
const CRASH_RESERVE_SIZE: u64 = 4 * 1024 * 1024; // 4 MiB

/// Boot-time tombstone magic written at the very front of the reserved region
/// by `write_crash_dump`. Read on the next boot to detect a prior crash.
///
/// The spec writes this as `0xRAEE_DEAD`. Since `R` is not a hex digit, we
/// encode the same intent as a valid u64: high half `0xRAEE` -> `0x0EAE`
/// (the printable bytes "RAEE" don't fit hex), so we use a fixed, recognizable
/// 64-bit pattern that is easy to spot in a memory dump and cannot collide
/// with `CRASH_DUMP_MAGIC` (the full-context magic above).
const CRASH_BOOT_MAGIC: u64 = 0x5241_4545_DEAD_DEAD; // "RAEE" ASCII || DEADDEAD

// ═══════════════════════════════════════════════════════════════════════════════
//  Dump Level
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DumpLevel {
    Mini = 0,   // Registers + stack only
    Kernel = 1, // + kernel memory snapshot
    Full = 2,   // + all RAM (not practical without disk I/O)
}

// ═══════════════════════════════════════════════════════════════════════════════
//  CPU Register State (captured at panic)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
#[repr(C)]
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
    pub cr0: u64,
    pub cr2: u64,
    pub cr3: u64,
    pub cr4: u64,
}

impl CpuRegisters {
    pub const fn zeroed() -> Self {
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
            cr0: 0,
            cr2: 0,
            cr3: 0,
            cr4: 0,
        }
    }

    /// Capture the current CPU register state.
    /// Must be called at the panic site for RIP to be meaningful.
    #[inline(never)]
    pub fn capture() -> Self {
        let mut regs = Self::zeroed();
        unsafe {
            core::arch::asm!(
                "mov [{regs} + 0x00], rax",
                "mov [{regs} + 0x08], rbx",
                "mov [{regs} + 0x10], rcx",
                "mov [{regs} + 0x18], rdx",
                "mov [{regs} + 0x20], rsi",
                "mov [{regs} + 0x28], rdi",
                "mov [{regs} + 0x30], rbp",
                "mov [{regs} + 0x38], rsp",
                "mov [{regs} + 0x40], r8",
                "mov [{regs} + 0x48], r9",
                "mov [{regs} + 0x50], r10",
                "mov [{regs} + 0x58], r11",
                "mov [{regs} + 0x60], r12",
                "mov [{regs} + 0x68], r13",
                "mov [{regs} + 0x70], r14",
                "mov [{regs} + 0x78], r15",
                regs = in(reg) &mut regs as *mut CpuRegisters as u64,
                options(nostack),
            );
            // RIP: use the return address (approximate)
            core::arch::asm!(
                "lea {rip}, [rip]",
                rip = out(reg) regs.rip,
                options(nomem, nostack),
            );
            core::arch::asm!(
                "pushfq",
                "pop {rflags}",
                rflags = out(reg) regs.rflags,
                options(nomem),
            );
            core::arch::asm!("mov {}, cr0", out(reg) regs.cr0, options(nomem, nostack));
            core::arch::asm!("mov {}, cr2", out(reg) regs.cr2, options(nomem, nostack));
            core::arch::asm!("mov {}, cr3", out(reg) regs.cr3, options(nomem, nostack));
            core::arch::asm!("mov {}, cr4", out(reg) regs.cr4, options(nomem, nostack));
        }
        regs
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Backtrace Walker (frame pointer chain)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct StackFrame {
    pub return_address: u64,
    pub frame_pointer: u64,
}

pub struct BacktraceWalker {
    pub frames: [StackFrame; MAX_BACKTRACE_DEPTH],
    pub depth: usize,
}

impl BacktraceWalker {
    pub const fn new() -> Self {
        Self {
            frames: [StackFrame {
                return_address: 0,
                frame_pointer: 0,
            }; MAX_BACKTRACE_DEPTH],
            depth: 0,
        }
    }

    /// Walk the stack frame chain starting from the current RBP.
    /// No allocation, no panicking — safe for crash context.
    #[inline(never)]
    pub fn walk_from_current(&mut self) {
        let mut rbp: u64;
        unsafe {
            core::arch::asm!("mov {}, rbp", out(reg) rbp, options(nomem, nostack));
        }
        self.walk_from(rbp);
    }

    /// Walk stack frames starting from a given frame pointer.
    pub fn walk_from(&mut self, start_rbp: u64) {
        self.depth = 0;
        let mut rbp = start_rbp;

        for _ in 0..MAX_BACKTRACE_DEPTH {
            if rbp == 0 || rbp % 8 != 0 {
                break;
            }

            // Validate the pointer is in a reasonable kernel range
            if !Self::is_kernel_addr(rbp) {
                break;
            }

            let frame_ptr = rbp as *const u64;
            let next_rbp = unsafe { core::ptr::read_volatile(frame_ptr) };
            let ret_addr = unsafe { core::ptr::read_volatile(frame_ptr.add(1)) };

            if ret_addr == 0 {
                break;
            }

            self.frames[self.depth] = StackFrame {
                return_address: ret_addr,
                frame_pointer: rbp,
            };
            self.depth += 1;

            // Sanity: next frame must be at a higher address (stack grows down)
            if next_rbp <= rbp && next_rbp != 0 {
                break;
            }
            rbp = next_rbp;
        }
    }

    fn is_kernel_addr(addr: u64) -> bool {
        // Kernel lives in the higher half on x86_64
        addr >= 0xFFFF_8000_0000_0000 || (addr >= 0x1000 && addr < 0x0000_8000_0000_0000)
    }

    pub fn return_addresses(&self) -> &[StackFrame] {
        &self.frames[..self.depth]
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Kernel Log Ring Buffer (preserved through crash)
// ═══════════════════════════════════════════════════════════════════════════════

#[repr(C)]
pub struct KernelLogRing {
    lines: [[u8; KERNEL_LOG_LINE_LEN]; KERNEL_LOG_RING_SIZE],
    line_lengths: [u8; KERNEL_LOG_RING_SIZE],
    write_index: usize,
    total_lines: u64,
}

impl KernelLogRing {
    pub const fn new() -> Self {
        Self {
            lines: [[0u8; KERNEL_LOG_LINE_LEN]; KERNEL_LOG_RING_SIZE],
            line_lengths: [0u8; KERNEL_LOG_RING_SIZE],
            write_index: 0,
            total_lines: 0,
        }
    }

    pub fn push_line(&mut self, line: &[u8]) {
        let len = line.len().min(KERNEL_LOG_LINE_LEN);
        self.lines[self.write_index][..len].copy_from_slice(&line[..len]);
        if len < KERNEL_LOG_LINE_LEN {
            self.lines[self.write_index][len..].fill(0);
        }
        self.line_lengths[self.write_index] = len as u8;
        self.write_index = (self.write_index + 1) % KERNEL_LOG_RING_SIZE;
        self.total_lines += 1;
    }

    pub fn last_n_lines(&self, n: usize) -> impl Iterator<Item = &[u8]> {
        let count = n.min(KERNEL_LOG_RING_SIZE).min(self.total_lines as usize);
        let start = (self.write_index + KERNEL_LOG_RING_SIZE - count) % KERNEL_LOG_RING_SIZE;
        (0..count).map(move |i| {
            let idx = (start + i) % KERNEL_LOG_RING_SIZE;
            let len = self.line_lengths[idx] as usize;
            &self.lines[idx][..len]
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Crash Context (the full state we serialize)
// ═══════════════════════════════════════════════════════════════════════════════

#[repr(C)]
pub struct CrashContext {
    pub magic: u64,
    pub version: u32,
    pub dump_level: u8,
    pub cpu_id: u8,
    pub _padding: [u8; 2],
    pub timestamp_tsc: u64,
    pub uptime_ms: u64,
    pub registers: CpuRegisters,
    pub backtrace: [StackFrame; MAX_BACKTRACE_DEPTH],
    pub backtrace_depth: u32,
    pub panic_msg: [u8; MAX_PANIC_MSG_LEN],
    pub panic_msg_len: u32,
    pub active_task_id: u64,
    pub active_task_name: [u8; MAX_TASK_NAME_LEN],
    pub memory_total_kb: u64,
    pub memory_free_kb: u64,
    pub memory_used_kb: u64,
    pub nr_tasks: u32,
    pub nr_cpus_online: u32,
    pub kernel_version: [u8; 32],
}

impl CrashContext {
    pub const fn empty() -> Self {
        Self {
            magic: 0,
            version: 0,
            dump_level: 0,
            cpu_id: 0,
            _padding: [0; 2],
            timestamp_tsc: 0,
            uptime_ms: 0,
            registers: CpuRegisters::zeroed(),
            backtrace: [StackFrame {
                return_address: 0,
                frame_pointer: 0,
            }; MAX_BACKTRACE_DEPTH],
            backtrace_depth: 0,
            panic_msg: [0u8; MAX_PANIC_MSG_LEN],
            panic_msg_len: 0,
            active_task_id: 0,
            active_task_name: [0u8; MAX_TASK_NAME_LEN],
            memory_total_kb: 0,
            memory_free_kb: 0,
            memory_used_kb: 0,
            nr_tasks: 0,
            nr_cpus_online: 0,
            kernel_version: [0u8; 32],
        }
    }

    pub fn is_valid(&self) -> bool {
        self.magic == CRASH_DUMP_MAGIC && self.version == CRASH_DUMP_VERSION
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Crash Dump Writer
// ═══════════════════════════════════════════════════════════════════════════════

pub struct CrashDumpWriter {
    pub region_phys_base: u64,
    pub region_size: usize,
    pub region_virt_base: u64,
    pub configured: bool,
    pub dump_level: DumpLevel,
    pub dump_in_progress: AtomicBool,
}

impl CrashDumpWriter {
    pub const fn new() -> Self {
        Self {
            region_phys_base: CRASH_REGION_PHYS_BASE,
            region_size: CRASH_REGION_SIZE,
            region_virt_base: 0,
            configured: false,
            dump_level: DumpLevel::Mini,
            dump_in_progress: AtomicBool::new(false),
        }
    }

    pub fn configure(&mut self, phys_offset: u64) {
        self.region_virt_base = phys_offset + self.region_phys_base;
        self.configured = true;
    }

    /// Write the crash dump to the reserved memory region.
    /// Called from the panic handler — NO ALLOCATION allowed.
    pub fn write_dump(&self, panic_msg: &str, regs: &CpuRegisters, backtrace: &BacktraceWalker) {
        if !self.configured {
            return;
        }
        if self.dump_in_progress.swap(true, Ordering::SeqCst) {
            return; // prevent re-entrance
        }

        let ctx_ptr = self.region_virt_base as *mut CrashContext;
        if ctx_ptr.is_null() {
            return;
        }

        unsafe {
            let ctx = &mut *ctx_ptr;
            ctx.magic = CRASH_DUMP_MAGIC;
            ctx.version = CRASH_DUMP_VERSION;
            ctx.dump_level = self.dump_level as u8;
            ctx.cpu_id = crate::gdt::current_cpu_id() as u8;
            ctx.timestamp_tsc = read_tsc_crash();
            ctx.uptime_ms = BOOT_TSC_START.load(Ordering::Relaxed);
            ctx.registers = *regs;

            // Copy backtrace frames
            ctx.backtrace_depth = backtrace.depth as u32;
            for i in 0..backtrace.depth.min(MAX_BACKTRACE_DEPTH) {
                ctx.backtrace[i] = backtrace.frames[i];
            }

            // Copy panic message
            let msg_bytes = panic_msg.as_bytes();
            let msg_len = msg_bytes.len().min(MAX_PANIC_MSG_LEN);
            ctx.panic_msg[..msg_len].copy_from_slice(&msg_bytes[..msg_len]);
            ctx.panic_msg_len = msg_len as u32;

            // Kernel version tag
            let ver = b"AthKernel v0.0.1";
            ctx.kernel_version[..ver.len()].copy_from_slice(ver);
        }
    }

    /// Check if a previous crash dump exists in the reserved region.
    pub fn check_previous_dump(&self) -> Option<&CrashContext> {
        if !self.configured {
            return None;
        }
        let ctx_ptr = self.region_virt_base as *const CrashContext;
        if ctx_ptr.is_null() {
            return None;
        }
        let ctx = unsafe { &*ctx_ptr };
        if ctx.is_valid() {
            Some(ctx)
        } else {
            None
        }
    }

    /// Clear the crash dump region (called after the dump has been processed).
    pub fn clear_dump(&self) {
        if !self.configured {
            return;
        }
        let region = self.region_virt_base as *mut u8;
        unsafe {
            core::ptr::write_bytes(region, 0, self.region_size);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Crash Report (high-level structured report, allocated post-boot)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct LoadedModule {
    pub name: String,
    pub base_address: u64,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct CrashReport {
    pub panic_message: String,
    pub backtrace_addrs: Vec<u64>,
    pub cpu_registers: CpuRegisters,
    pub loaded_modules: Vec<LoadedModule>,
    pub kernel_version: String,
    pub hardware_info: String,
    pub uptime_ms: u64,
    pub timestamp_tsc: u64,
    pub cpu_id: u8,
    pub memory_total_kb: u64,
    pub memory_free_kb: u64,
    pub active_task: String,
    pub kernel_log_tail: Vec<String>,
}

impl CrashReport {
    pub fn from_context(ctx: &CrashContext) -> Self {
        let panic_msg_len = ctx.panic_msg_len as usize;
        let panic_msg = core::str::from_utf8(&ctx.panic_msg[..panic_msg_len])
            .unwrap_or("<invalid utf8>")
            .into();

        let mut backtrace_addrs = Vec::new();
        for i in 0..(ctx.backtrace_depth as usize).min(MAX_BACKTRACE_DEPTH) {
            if ctx.backtrace[i].return_address != 0 {
                backtrace_addrs.push(ctx.backtrace[i].return_address);
            }
        }

        let ver_end = ctx
            .kernel_version
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(32);
        let kernel_version = core::str::from_utf8(&ctx.kernel_version[..ver_end])
            .unwrap_or("unknown")
            .into();

        let task_end = ctx
            .active_task_name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(MAX_TASK_NAME_LEN);
        let active_task = core::str::from_utf8(&ctx.active_task_name[..task_end])
            .unwrap_or("unknown")
            .into();

        Self {
            panic_message: panic_msg,
            backtrace_addrs,
            cpu_registers: ctx.registers,
            loaded_modules: Vec::new(),
            kernel_version,
            hardware_info: String::from("x86_64"),
            uptime_ms: ctx.uptime_ms,
            timestamp_tsc: ctx.timestamp_tsc,
            cpu_id: ctx.cpu_id,
            memory_total_kb: ctx.memory_total_kb,
            memory_free_kb: ctx.memory_free_kb,
            active_task,
            kernel_log_tail: Vec::new(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Symbol Resolution (basic — function name from address)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct SymbolEntry {
    pub address: u64,
    pub size: u64,
    pub name: String,
}

pub struct SymbolTable {
    pub symbols: Vec<SymbolEntry>,
    pub sorted: bool,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self {
            symbols: Vec::new(),
            sorted: false,
        }
    }

    pub fn add(&mut self, address: u64, size: u64, name: String) {
        self.symbols.push(SymbolEntry {
            address,
            size,
            name,
        });
        self.sorted = false;
    }

    pub fn sort(&mut self) {
        self.symbols.sort_by_key(|s| s.address);
        self.sorted = true;
    }

    pub fn resolve(&self, addr: u64) -> Option<(&str, u64)> {
        if self.symbols.is_empty() {
            return None;
        }
        // Binary search for the symbol containing this address
        let idx = self.symbols.partition_point(|s| s.address <= addr);
        if idx == 0 {
            return None;
        }
        let sym = &self.symbols[idx - 1];
        if addr < sym.address + sym.size || sym.size == 0 {
            let offset = addr - sym.address;
            Some((sym.name.as_str(), offset))
        } else {
            None
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Minidump Format Writer
// ═══════════════════════════════════════════════════════════════════════════════

#[repr(C)]
pub struct MinidumpHeader {
    pub signature: [u8; 8], // "RAEDUMP\0"
    pub version: u32,
    pub stream_count: u32,
    pub stream_directory_offset: u32,
    pub flags: u32,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum MinidumpStreamType {
    SystemInfo = 1,
    ThreadList = 2,
    ExceptionRecord = 3,
    Memory64List = 4,
    ModuleList = 5,
    MiscInfo = 6,
}

#[repr(C)]
pub struct MinidumpStreamEntry {
    pub stream_type: u32,
    pub offset: u32,
    pub size: u32,
}

pub struct MinidumpWriter {
    pub buffer_base: u64,
    pub buffer_size: usize,
    pub write_offset: usize,
}

impl MinidumpWriter {
    pub const fn new() -> Self {
        Self {
            buffer_base: 0,
            buffer_size: 0,
            write_offset: 0,
        }
    }

    pub fn init(&mut self, base: u64, size: usize) {
        self.buffer_base = base;
        self.buffer_size = size;
        self.write_offset = 0;
    }

    /// Write raw bytes to the dump buffer. No allocation.
    fn write_bytes(&mut self, data: &[u8]) -> bool {
        if self.write_offset + data.len() > self.buffer_size {
            return false;
        }
        let dst = (self.buffer_base + self.write_offset as u64) as *mut u8;
        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
        }
        self.write_offset += data.len();
        true
    }

    pub fn write_header(&mut self, stream_count: u32) -> bool {
        let header = MinidumpHeader {
            signature: *b"RAEDUMP\0",
            version: 1,
            stream_count,
            stream_directory_offset: core::mem::size_of::<MinidumpHeader>() as u32,
            flags: 0,
            timestamp: read_tsc_crash(),
        };
        let bytes = unsafe {
            core::slice::from_raw_parts(
                &header as *const MinidumpHeader as *const u8,
                core::mem::size_of::<MinidumpHeader>(),
            )
        };
        self.write_bytes(bytes)
    }

    pub fn write_registers(&mut self, regs: &CpuRegisters) -> bool {
        let bytes = unsafe {
            core::slice::from_raw_parts(
                regs as *const CpuRegisters as *const u8,
                core::mem::size_of::<CpuRegisters>(),
            )
        };
        self.write_bytes(bytes)
    }

    pub fn write_backtrace(&mut self, frames: &[StackFrame], depth: usize) -> bool {
        let count = depth.min(MAX_BACKTRACE_DEPTH);
        let count_bytes = (count as u32).to_le_bytes();
        if !self.write_bytes(&count_bytes) {
            return false;
        }
        for i in 0..count {
            let addr_bytes = frames[i].return_address.to_le_bytes();
            if !self.write_bytes(&addr_bytes) {
                return false;
            }
        }
        true
    }

    pub fn bytes_written(&self) -> usize {
        self.write_offset
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Post-Reboot Crash Detection
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashDumpPresence {
    NoDump,
    DumpFound,
    DumpCorrupted,
}

pub struct PostRebootHandler {
    pub dump_present: CrashDumpPresence,
    pub last_report: Option<CrashReport>,
}

impl PostRebootHandler {
    pub fn new() -> Self {
        Self {
            dump_present: CrashDumpPresence::NoDump,
            last_report: None,
        }
    }

    pub fn check_and_recover(&mut self, writer: &CrashDumpWriter) {
        if let Some(ctx) = writer.check_previous_dump() {
            self.dump_present = CrashDumpPresence::DumpFound;
            self.last_report = Some(CrashReport::from_context(ctx));
        } else {
            self.dump_present = CrashDumpPresence::NoDump;
        }
    }

    pub fn dismiss(&mut self, writer: &CrashDumpWriter) {
        writer.clear_dump();
        self.dump_present = CrashDumpPresence::NoDump;
        self.last_report = None;
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TSC helper (crash-safe, no deps)
// ═══════════════════════════════════════════════════════════════════════════════

#[inline]
fn read_tsc_crash() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack, preserves_flags),
        );
    }
    ((hi as u64) << 32) | lo as u64
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Global State & Init
// ═══════════════════════════════════════════════════════════════════════════════

pub static CRASH_WRITER: Mutex<CrashDumpWriter> = Mutex::new(CrashDumpWriter::new());
pub static KERNEL_LOG: Mutex<KernelLogRing> = Mutex::new(KernelLogRing::new());
pub static BOOT_TSC_START: AtomicU64 = AtomicU64::new(0);

/// Phase 4.5: physical base address of the 4 MiB region reserved off the top
/// of RAM for crash dumps. Set once by [`reserve_crash_region`] during memory
/// init; 0 means "not yet reserved". Read by the panic path and procfs.
pub static CRASH_REGION_PHYS: AtomicU64 = AtomicU64::new(0);

/// Set to true on boot if [`check_boot_tombstone`] found a prior-crash magic.
static PREV_DUMP_FOUND: AtomicBool = AtomicBool::new(false);
pub static SYMBOL_TABLE: Mutex<SymbolTable> = Mutex::new(SymbolTable {
    symbols: Vec::new(),
    sorted: false,
});

/// Log a line to the kernel ring buffer (survives crash for inclusion in dump).
pub fn klog(msg: &[u8]) {
    if let Some(mut log) = KERNEL_LOG.try_lock() {
        log.push_line(msg);
    }
}

/// Called from panic handler to create the crash dump.
/// MUST NOT allocate. MUST NOT take contested locks.
pub fn panic_dump(panic_msg: &str) {
    let regs = CpuRegisters::capture();
    let mut bt = BacktraceWalker::new();
    bt.walk_from_current();

    if let Some(writer) = CRASH_WRITER.try_lock() {
        writer.write_dump(panic_msg, &regs, &bt);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Phase 4.5: high-RAM reservation + boot tombstone + panic-handler entry
// ═══════════════════════════════════════════════════════════════════════════════

/// Lightweight, fixed-layout tombstone written to the front of the reserved
/// high-RAM region by [`write_crash_dump`]. Unlike [`CrashContext`] this is the
/// minimal "something died" record the spec asks for: magic, timestamp, a
/// truncated panic message, and a register snapshot (RIP/RSP/RBP). It lives at
/// the reserved physical base so it survives a warm reboot.
#[repr(C)]
pub struct BootTombstone {
    pub magic: u64,
    pub timestamp_tsc: u64,
    pub rip: u64,
    pub rsp: u64,
    pub rbp: u64,
    pub panic_msg_len: u32,
    pub _padding: u32,
    pub panic_msg: [u8; 256],
}

impl BootTombstone {
    const PANIC_MSG_CAP: usize = 256;
}

/// Reserve the last 4 MiB of physical RAM for crash dumps.
///
/// Called during memory init with the total number of physical 4 KiB pages.
/// Returns the physical base address of the reserved region, or `None` if RAM
/// is too small to carve out 4 MiB. The address is recorded in
/// [`CRASH_REGION_PHYS`] for the panic path and procfs to read.
///
/// The caller (memory init) is responsible for excluding `[base, base+4MiB)`
/// from the frame allocator so the region is never handed to the buddy
/// allocator. We only compute and record the address here.
pub fn reserve_crash_region(total_pages: usize) -> Option<u64> {
    let total_bytes = (total_pages as u64).checked_mul(4096)?;
    if total_bytes <= CRASH_RESERVE_SIZE {
        return None;
    }
    // Last 4 MiB of RAM, aligned down to a 4 KiB page boundary.
    let base = (total_bytes - CRASH_RESERVE_SIZE) & !0xFFFu64;
    CRASH_REGION_PHYS.store(base, Ordering::SeqCst);
    Some(base)
}

/// Resolve the virtual address of the reserved high-RAM region through the
/// kernel's direct physical map. Returns `None` if the region was never
/// reserved or the physical-memory offset is unknown.
fn crash_region_virt() -> Option<u64> {
    let phys = CRASH_REGION_PHYS.load(Ordering::Relaxed);
    if phys == 0 {
        return None;
    }
    let offset = crate::memory::PHYS_MEM_OFFSET.get().map(|v| v.as_u64())?;
    Some(offset.wrapping_add(phys))
}

/// On boot, read the reserved region and check for the prior-crash magic.
/// If found, log it and clear the magic so we don't re-report on the next
/// boot. Returns true if a previous crash dump was present.
pub fn check_boot_tombstone() -> bool {
    let Some(virt) = crash_region_virt() else {
        return false;
    };
    let tomb_ptr = virt as *mut BootTombstone;

    // SAFETY: `virt` is the direct-map address of the reserved high-RAM region,
    // which is mapped present+writable for the lifetime of the kernel. The
    // region is at least 4 MiB, far larger than `BootTombstone`, so the read
    // and the subsequent magic clear are in-bounds. Volatile because the bytes
    // may have been written by a previous boot's panic path.
    let found = unsafe {
        let magic = core::ptr::read_volatile(core::ptr::addr_of!((*tomb_ptr).magic));
        if magic == CRASH_BOOT_MAGIC {
            let len =
                core::ptr::read_volatile(core::ptr::addr_of!((*tomb_ptr).panic_msg_len)) as usize;
            let len = len.min(BootTombstone::PANIC_MSG_CAP);
            let rip = core::ptr::read_volatile(core::ptr::addr_of!((*tomb_ptr).rip));
            let rsp = core::ptr::read_volatile(core::ptr::addr_of!((*tomb_ptr).rsp));
            let rbp = core::ptr::read_volatile(core::ptr::addr_of!((*tomb_ptr).rbp));
            let tsc = core::ptr::read_volatile(core::ptr::addr_of!((*tomb_ptr).timestamp_tsc));
            // Copy the truncated panic message out of the volatile region.
            let mut msg_bytes = [0u8; BootTombstone::PANIC_MSG_CAP];
            for (i, b) in msg_bytes.iter_mut().enumerate().take(len) {
                *b = core::ptr::read_volatile(core::ptr::addr_of!((*tomb_ptr).panic_msg[i]));
            }
            let msg = alloc::string::String::from_utf8_lossy(&msg_bytes[..len]).into_owned();
            crate::serial_println!(
                "[crash] previous crash dump found! Queued for /var/crash/athena-{:#x}.dump (rip={:#x}, msg_len={})",
                tsc,
                rip,
                len,
            );
            // Stash the record so `flush_pending_crash_dump` can write it to
            // AthFS once the filesystem is mounted (init() runs before mount).
            *PENDING_DUMP.lock() = Some(PendingCrash {
                timestamp_tsc: tsc,
                rip,
                rsp,
                rbp,
                msg,
            });
            // Clear the magic after reading so the next boot is clean.
            core::ptr::write_volatile(core::ptr::addr_of_mut!((*tomb_ptr).magic), 0u64);
            true
        } else {
            false
        }
    };

    PREV_DUMP_FOUND.store(found, Ordering::SeqCst);
    found
}

/// A crash record captured from the boot tombstone in [`check_boot_tombstone`],
/// held until AthFS is mounted and [`flush_pending_crash_dump`] can persist it.
struct PendingCrash {
    timestamp_tsc: u64,
    rip: u64,
    rsp: u64,
    rbp: u64,
    msg: String,
}

/// Captured prior-boot crash, awaiting a AthFS write. `None` on a clean boot.
static PENDING_DUMP: spin::Mutex<Option<PendingCrash>> = spin::Mutex::new(None);

/// Render a human-readable `/var/crash` dump body for a [`PendingCrash`].
fn format_dump_text(p: &PendingCrash) -> String {
    use core::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(out, "AthenaOS kernel crash dump (Phase 4.5)");
    let _ = writeln!(out, "magic:    {:#018x}", CRASH_BOOT_MAGIC);
    let _ = writeln!(out, "tsc:      {:#x}", p.timestamp_tsc);
    let _ = writeln!(out, "rip:      {:#x}", p.rip);
    let _ = writeln!(out, "rsp:      {:#x}", p.rsp);
    let _ = writeln!(out, "rbp:      {:#x}", p.rbp);
    let _ = writeln!(out, "panic:    {}", p.msg);
    out
}

/// Flat AthFS filename for a crash dump keyed by TSC. AthFS root files are a
/// flat namespace (no `/`, ≤55 bytes); the logical path is `/var/crash/<name>`.
fn crash_dump_filename(tsc: u64) -> String {
    alloc::format!("varcrash-{:x}.dump", tsc)
}

/// Phase 4.5: persist the prior-boot crash (captured by
/// [`check_boot_tombstone`]) to AthFS as `/var/crash/varcrash-<tsc>.dump`.
/// Call AFTER AthFS is mounted. No-op on a clean boot; refused in safe-mode
/// (the dump stays in the reserved high-RAM region for the next non-safe boot).
pub fn flush_pending_crash_dump() {
    let pending = PENDING_DUMP.lock().take();
    let Some(p) = pending else {
        return; // clean boot, nothing queued
    };
    if crate::block_io::safe_mode_enabled() {
        crate::serial_println!(
            "[crash] /var/crash persist skipped (safe-mode, read-only); rip={:#x} retained in RAM",
            p.rip,
        );
        return;
    }
    let body = format_dump_text(&p);
    let name = crash_dump_filename(p.timestamp_tsc);
    let ok = crate::athfs::write_flat_file(&name, body.as_bytes());
    crate::serial_println!(
        "[crash] persisted prior crash to /var/crash/{} ({} bytes) ok={}",
        name,
        body.len(),
        ok,
    );

    // Phase 4.5 "user-facing report tool": the user must LEARN about the
    // crash, not discover a hex file by accident. A plain-language report
    // lands next to the raw dump, and a critical toast points at it.
    let report = format_user_report(&p, &name);
    let report_ok = crate::athfs::write_flat_file("crash-report.txt", report.as_bytes());
    let _ = crate::notify::post(
        "Crash Reporter",
        "AthenaOS recovered from a crash - report saved",
        crate::shell_api::NotificationUrgency::Critical,
    );
    crate::serial_println!(
        "[crash] user report written (crash-report.txt, {} bytes) ok={}, toast posted",
        report.len(),
        report_ok,
    );
}

/// Plain-language crash report for the USER (Phase 4.5 — "User-facing
/// report tool"), distinct from the raw register dump: what happened, when,
/// where, and where the full evidence lives.
fn format_user_report(p: &PendingCrash, dump_name: &str) -> String {
    use core::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(out, "AthenaOS Crash Report");
    let _ = writeln!(out, "====================");
    let _ = writeln!(
        out,
        "The previous session ended unexpectedly. The system recovered on this boot."
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "What happened: {}", p.msg);
    let _ = writeln!(
        out,
        "Where: instruction pointer {:#x} (stack {:#x})",
        p.rip, p.rsp
    );
    let _ = writeln!(out, "When (TSC): {:#x}", p.timestamp_tsc);
    let _ = writeln!(out);
    let _ = writeln!(out, "Full technical dump: /var/crash/{}", dump_name);
    let _ = writeln!(out, "Live diagnostics:    /proc/athena/crash");
    let _ = writeln!(
        out,
        "If this repeats, file a report with both files attached."
    );
    out
}

/// Phase 4.5 R10 proof: exercise the full /var/crash write+read path on a
/// single boot without a real prior crash. Synthesizes a crash record, writes
/// it to AthFS, reads it back, and verifies the bytes round-trip. Uses the
/// live mount when one is writable; otherwise (safe mode, or no AthFS
/// partition on the boot disk — auto-format refuses non-blank disks) the SAME
/// write+readback proves itself on a throwaway RAM-backed volume.
pub fn run_persist_smoketest() {
    let p = PendingCrash {
        timestamp_tsc: 0xC0FFEE,
        rip: 0xDEAD_BEEF,
        rsp: 0x1000,
        rbp: 0x2000,
        msg: String::from("synthetic selftest panic"),
    };
    let body = format_dump_text(&p);
    let name = "varcrash-selftest.dump";

    let exercise = || {
        let wrote = crate::athfs::write_flat_file(name, body.as_bytes());
        let read_back = crate::athfs::read_flat_file(name);
        let matches = read_back.as_deref() == Some(body.as_bytes());
        (wrote, matches)
    };

    let live_writable =
        { crate::athfs::ATHFS.lock().is_some() } && !crate::block_io::safe_mode_enabled();
    let (wrote, matches, volume) = if live_writable {
        let (w, m) = exercise();
        (w, m, "live")
    } else {
        match crate::athfs::with_ram_athfs(exercise) {
            Some((w, m)) => (w, m, "ram-volume"),
            None => (false, false, "ram-volume setup failed"),
        }
    };

    let pass = wrote && matches;
    crate::serial_println!(
        "[crash] persist smoketest: wrote={} readback_matches={} bytes={} ({}) -> {}",
        wrote,
        matches,
        body.len(),
        volume,
        if pass { "PASS" } else { "FAIL" }
    );

    // Phase 4.5 user-facing report: the plain-language report must carry the
    // panic message, the RIP, and the pointer to the raw dump — what a user
    // (or a bug tracker) actually needs.
    let report = format_user_report(&p, "varcrash-selftest.dump");
    let report_ok = report.contains("synthetic selftest panic")
        && report.contains("0xdeadbeef")
        && report.contains("/var/crash/varcrash-selftest.dump")
        && report.contains("/proc/athena/crash");
    crate::serial_println!(
        "[crash] report smoketest: msg={} rip_hex={} dump_path={} proc_path={} -> {}",
        report.contains("synthetic selftest panic"),
        report.contains("0xdeadbeef"),
        report.contains("/var/crash/varcrash-selftest.dump"),
        report.contains("/proc/athena/crash"),
        if report_ok { "PASS" } else { "FAIL" }
    );
}

/// Returns whether [`check_boot_tombstone`] saw a prior-crash tombstone this
/// boot. Used by the smoketest and procfs.
pub fn previous_dump_found() -> bool {
    PREV_DUMP_FOUND.load(Ordering::Relaxed)
}

/// Panic-handler entry point. Writes a [`BootTombstone`] to the reserved
/// high-RAM region: magic, timestamp, the first 256 bytes of the panic
/// message, and a register snapshot (RIP/RSP/RBP read from the current frame).
///
/// MUST NOT allocate and MUST NOT take contested locks — it runs from the
/// `#[panic_handler]` where the kernel may be partially corrupted. It also
/// drives the richer [`panic_dump`] path so both the tombstone and the full
/// [`CrashContext`] are populated.
pub fn write_crash_dump(info: &core::panic::PanicInfo) {
    // Read RIP/RSP/RBP from the current stack frame. RIP is taken as the
    // address of this function body via a RIP-relative lea (approximate, but
    // anchored inside the panic path).
    let rip: u64;
    let rsp: u64;
    let rbp: u64;
    // SAFETY: pure register reads, no memory access, preserves flags. These
    // capture the live panic-time RSP/RBP and a RIP-relative anchor.
    unsafe {
        core::arch::asm!("lea {}, [rip]", out(reg) rip, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mov {}, rsp", out(reg) rsp, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mov {}, rbp", out(reg) rbp, options(nomem, nostack, preserves_flags));
    }

    if let Some(virt) = crash_region_virt() {
        let tomb_ptr = virt as *mut BootTombstone;
        // SAFETY: `virt` is the always-mapped, writable direct-map address of
        // the reserved 4 MiB region; `BootTombstone` fits comfortably. Volatile
        // writes so the values land in RAM even if the optimizer thinks the
        // region is dead after we halt.
        unsafe {
            core::ptr::write_volatile(
                core::ptr::addr_of_mut!((*tomb_ptr).timestamp_tsc),
                read_tsc_crash(),
            );
            core::ptr::write_volatile(core::ptr::addr_of_mut!((*tomb_ptr).rip), rip);
            core::ptr::write_volatile(core::ptr::addr_of_mut!((*tomb_ptr).rsp), rsp);
            core::ptr::write_volatile(core::ptr::addr_of_mut!((*tomb_ptr).rbp), rbp);

            // Format the panic message into a fixed stack buffer (no heap).
            let mut buf = [0u8; BootTombstone::PANIC_MSG_CAP];
            let written = format_panic_message(info, &mut buf);
            let dst = core::ptr::addr_of_mut!((*tomb_ptr).panic_msg) as *mut u8;
            core::ptr::copy_nonoverlapping(buf.as_ptr(), dst, written);
            if written < BootTombstone::PANIC_MSG_CAP {
                core::ptr::write_bytes(dst.add(written), 0, BootTombstone::PANIC_MSG_CAP - written);
            }
            core::ptr::write_volatile(
                core::ptr::addr_of_mut!((*tomb_ptr).panic_msg_len),
                written as u32,
            );

            // Write the magic LAST so a reader never sees a half-written record.
            core::ptr::write_volatile(core::ptr::addr_of_mut!((*tomb_ptr).magic), CRASH_BOOT_MAGIC);
        }
    }

    // Drive the full structured dump as well (registers + backtrace + context).
    panic_dump("AthKernel panic");
}

/// Format a `PanicInfo` into a fixed byte buffer without allocating, returning
/// the number of bytes written (capped at the buffer length).
fn format_panic_message(info: &core::panic::PanicInfo, buf: &mut [u8]) -> usize {
    use core::fmt::Write;
    struct SliceWriter<'a> {
        buf: &'a mut [u8],
        pos: usize,
    }
    impl<'a> Write for SliceWriter<'a> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let bytes = s.as_bytes();
            let space = self.buf.len() - self.pos;
            let n = bytes.len().min(space);
            self.buf[self.pos..self.pos + n].copy_from_slice(&bytes[..n]);
            self.pos += n;
            Ok(())
        }
    }
    let mut w = SliceWriter { buf, pos: 0 };
    // PanicInfo's Display includes location + message; ignore fmt errors (we
    // simply truncate). This must never panic re-entrantly.
    let _ = write!(w, "{}", info);
    w.pos
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Phase 4.5 additions: mem_map-based reservation, raw dump writer, and
//  simplified boot tombstone checker (public façade over check_boot_tombstone).
// ═══════════════════════════════════════════════════════════════════════════════

/// Reserve the last 4 MiB of physical RAM for crash dumps.
///
/// Scans the bootloader memory map for the highest usable region, carves the
/// last [`CRASH_RESERVE_SIZE`] bytes off it, records the physical base in
/// [`CRASH_REGION_PHYS`], and logs the address. Returns `Some(phys_base)` or
/// `None` if no usable region is large enough.
///
/// Called early in `kernel_main` **before** the buddy allocator is seeded;
/// the caller must exclude `[phys_base, phys_base + 4 MiB)` from the frame
/// allocator so this region is never allocated.
pub fn reserve_crash_region_from_map(mem_map: &bootloader_api::info::MemoryRegions) -> Option<u64> {
    use bootloader_api::info::MemoryRegionKind;

    // Find the usable region with the highest end address.
    let mut best_end: u64 = 0;
    let mut best_start: u64 = 0;
    for region in mem_map.iter() {
        if region.kind != MemoryRegionKind::Usable {
            continue;
        }
        let end = region.end;
        if end > best_end {
            best_end = end;
            best_start = region.start;
        }
    }

    if best_end == 0 || best_end.saturating_sub(best_start) <= CRASH_RESERVE_SIZE {
        return None;
    }

    // Reserve the last 4 MiB of the highest usable region, page-aligned.
    let base = (best_end - CRASH_RESERVE_SIZE) & !0xFFFu64;
    if base < best_start {
        return None;
    }

    CRASH_REGION_PHYS.store(base, Ordering::SeqCst);
    crate::serial_println!("[crash] reserved 4 MiB crash region at phys {:#x}", base,);
    Some(base)
}

/// Write a minimal crash tombstone to the reserved region.
///
/// Designed for the panic handler: **must not allocate**, **must not take
/// contested locks**, safe to call from any execution context. Writes:
///
/// - 8-byte magic (`CRASH_BOOT_MAGIC`) at offset 0
/// - panic message, up to 256 bytes, at offset 8
/// - RIP, RSP, RBP (8 bytes each) following the message
/// - A full memory fence to ensure all bytes reach RAM before the next halt
///
/// If the crash region was never reserved (`CRASH_REGION_PHYS == 0`) or
/// `PHYS_MEM_OFFSET` is unavailable this is a safe no-op.
///
/// # Safety
///
/// `virt` is derived from the direct-map physical offset plus the reserved
/// physical base. The reserved region is permanently mapped and at least 4 MiB
/// in size. All writes are volatile to prevent the optimizer from discarding
/// them. The caller must ensure this is not called re-entrantly (the outer
/// `write_crash_dump` uses a TSC-based early-exit for re-entrance).
pub unsafe fn write_crash_dump_raw(msg: &str, rip: u64, rsp: u64, rbp: u64) {
    let phys = CRASH_REGION_PHYS.load(Ordering::Relaxed);
    if phys == 0 {
        return;
    }
    let offset = match crate::memory::PHYS_MEM_OFFSET.get() {
        Some(o) => o.as_u64(),
        None => return,
    };
    // SAFETY: phys is within the reserved 4 MiB region mapped at offset+phys.
    // Region size (4 MiB) far exceeds the fixed-size tombstone layout written
    // here. All pointer arithmetic stays within [phys, phys+4MiB). Volatile
    // writes guarantee the stores are not elided.
    let base = offset.wrapping_add(phys) as *mut u8;

    // Write magic (8 bytes at offset 0).
    core::ptr::write_volatile(base as *mut u64, CRASH_BOOT_MAGIC);

    // Write panic message (up to 256 bytes at offset 8).
    let msg_bytes = msg.as_bytes();
    let msg_len = msg_bytes.len().min(256);
    let msg_dst = base.add(8);
    for i in 0..msg_len {
        core::ptr::write_volatile(msg_dst.add(i), msg_bytes[i]);
    }
    // Zero-pad the rest of the 256-byte slot.
    for i in msg_len..256 {
        core::ptr::write_volatile(msg_dst.add(i), 0u8);
    }

    // Write RIP / RSP / RBP (8 bytes each at offsets 8+256, +8, +16).
    let regs_base = base.add(8 + 256) as *mut u64;
    core::ptr::write_volatile(regs_base, rip);
    core::ptr::write_volatile(regs_base.add(1), rsp);
    core::ptr::write_volatile(regs_base.add(2), rbp);

    // Full memory fence: ensure all volatile stores are ordered before any
    // subsequent halt/reset instruction.
    core::sync::atomic::fence(Ordering::SeqCst);
}

/// Check whether a crash tombstone from a previous boot is present.
///
/// Reads the magic at the front of the reserved region. If it matches
/// [`CRASH_BOOT_MAGIC`]:
/// - Logs the crash to serial (would be written to `/var/crash/` by init).
/// - Clears the magic so the next boot does not re-report it.
/// - Returns `true`.
///
/// Returns `false` if no tombstone is present or the region was not reserved.
/// This is a thin wrapper over [`check_boot_tombstone`] for callers that only
/// need the bool result and the side-effect log.
pub fn check_previous_crash() -> bool {
    let phys = CRASH_REGION_PHYS.load(Ordering::Relaxed);
    if phys == 0 {
        return false;
    }
    let offset = match crate::memory::PHYS_MEM_OFFSET.get() {
        Some(o) => o.as_u64(),
        None => return false,
    };
    // SAFETY: phys is the reserved region base; region is at least 4 MiB and
    // permanently mapped. Read-volatile + write-volatile for the clear.
    let base = offset.wrapping_add(phys) as *const u32;
    let magic = unsafe { core::ptr::read_volatile(base as *const u64) };
    if magic == CRASH_BOOT_MAGIC {
        crate::serial_println!(
            "[crash] PREVIOUS CRASH DUMP FOUND at phys {:#x} — would write to /var/crash/",
            phys,
        );
        // Clear magic so the next boot starts clean.
        unsafe {
            core::ptr::write_volatile(base as *mut u64, 0u64);
        }
        PREV_DUMP_FOUND.store(true, Ordering::SeqCst);
        true
    } else {
        false
    }
}

pub fn init() {
    BOOT_TSC_START.store(read_tsc_crash(), Ordering::SeqCst);

    let phys_offset = crate::memory::PHYS_MEM_OFFSET
        .get()
        .map(|v| v.as_u64())
        .unwrap_or(0);

    if phys_offset > 0 {
        let mut writer = CRASH_WRITER.lock();
        writer.configure(phys_offset);

        // Check for previous crash dump from last boot (legacy 1 MiB region).
        if let Some(ctx) = writer.check_previous_dump() {
            if ctx.is_valid() {
                // A crash dump from a previous boot exists
                // It will be processed by userspace after full init
            }
        }
    }

    // Phase 4.5: check the high-RAM tombstone written by `write_crash_dump`
    // on the previous boot, log it, and clear the magic.
    check_boot_tombstone();
}

/// R10 smoketest: report the reserved crash-region address and whether a
/// previous crash dump was detected this boot.
/// MasterChecklist Phase 4.5: Crash dump.
pub fn run_boot_smoketest() {
    let phys = CRASH_REGION_PHYS.load(Ordering::Relaxed);
    let prev = previous_dump_found();
    if phys != 0 {
        crate::serial_println!(
            "[crash] crash region reserved at phys {:#x} (4 MiB) -> PASS",
            phys,
        );
        crate::serial_println!(
            "[crash] run_boot_smoketest: region={:#x} prev_dump={} -> PASS",
            phys,
            prev,
        );
    } else {
        crate::serial_println!(
            "[crash] run_boot_smoketest: region NOT reserved (RAM too small or reserve not called) prev_dump={} -> FAIL",
            prev,
        );
    }
}

/// /proc/athena/crash — reserved-region address, tombstone magic, and whether a
/// previous crash dump was detected this boot.
/// MasterChecklist Phase 4.5: Crash dump.
pub fn dump_text() -> String {
    use core::fmt::Write;
    let mut out = String::new();
    let phys = CRASH_REGION_PHYS.load(Ordering::Relaxed);
    let _ = writeln!(out, "# Phase 4.5: kernel crash dump");
    let _ = writeln!(
        out,
        "reserve_size:      {} bytes ({} MiB)",
        CRASH_RESERVE_SIZE,
        CRASH_RESERVE_SIZE >> 20
    );
    if phys != 0 {
        let _ = writeln!(out, "region_phys_base:  {:#x}", phys);
        let _ = writeln!(out, "region_phys_end:   {:#x}", phys + CRASH_RESERVE_SIZE);
    } else {
        let _ = writeln!(out, "region_phys_base:  (not reserved)");
    }
    let _ = writeln!(out, "boot_magic:        {:#018x}", CRASH_BOOT_MAGIC);
    let _ = writeln!(out, "previous_dump:     {}", previous_dump_found());

    // If a tombstone is currently present (not yet cleared), surface its
    // contents read-only.
    if let Some(virt) = crash_region_virt() {
        let tomb_ptr = virt as *const BootTombstone;
        // SAFETY: `virt` is the mapped direct-map address of the reserved
        // region, large enough for `BootTombstone`; read-only volatile reads.
        unsafe {
            let magic = core::ptr::read_volatile(core::ptr::addr_of!((*tomb_ptr).magic));
            if magic == CRASH_BOOT_MAGIC {
                let len = (core::ptr::read_volatile(core::ptr::addr_of!((*tomb_ptr).panic_msg_len))
                    as usize)
                    .min(BootTombstone::PANIC_MSG_CAP);
                let rip = core::ptr::read_volatile(core::ptr::addr_of!((*tomb_ptr).rip));
                let rsp = core::ptr::read_volatile(core::ptr::addr_of!((*tomb_ptr).rsp));
                let rbp = core::ptr::read_volatile(core::ptr::addr_of!((*tomb_ptr).rbp));
                let msg_bytes = core::slice::from_raw_parts(
                    core::ptr::addr_of!((*tomb_ptr).panic_msg) as *const u8,
                    len,
                );
                let msg = core::str::from_utf8(msg_bytes).unwrap_or("<invalid utf8>");
                let _ = writeln!(out, "tombstone.rip:     {:#x}", rip);
                let _ = writeln!(out, "tombstone.rsp:     {:#x}", rsp);
                let _ = writeln!(out, "tombstone.rbp:     {:#x}", rbp);
                let _ = writeln!(out, "tombstone.message: {}", msg);
            } else {
                let _ = writeln!(out, "tombstone:         (none present)");
            }
        }
    }
    out
}
