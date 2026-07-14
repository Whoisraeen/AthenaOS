//! Kernel Security Hardening — KASLR, stack canaries, CFI, KASAN, KFENCE,
//! UBSAN, W^X enforcement, kernel lockdown, SMAP/SMEP, Spectre/Meltdown mitigations.
//!
//! Concept §Security: AthenaOS must ship defense-in-depth hardening AND must not
//! lie about it. This module reports only what is genuinely instrumenting code.
//! KASAN/KFENCE/KCFI require compiler/build support that is not on by default,
//! so they are gated behind build features and reported honestly (see
//! `honesty_audit()` and the HONESTY NOTICE in `dump_text()`).
//!
//! R10 contract: `init()` (kernel_main) + `run_boot_smoketest()` +
//! `dump_text()`/`status()` for `/proc/raeen/hardening`.
#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

// ===========================================================================
// KASLR — Kernel Address Space Layout Randomization
// ===========================================================================

const KASLR_ENTROPY_BITS: u32 = 18;
const KASLR_ALIGN: u64 = 0x200000; // 2 MiB alignment for kernel base
const KASLR_MAX_SLIDE: u64 = (1 << KASLR_ENTROPY_BITS) * KASLR_ALIGN;

#[derive(Debug, Clone, Copy)]
pub struct KaslrLayout {
    pub kernel_base: u64,
    pub kernel_slide: u64,
    pub module_base: u64,
    pub module_slide: u64,
    pub stack_base: u64,
    pub stack_slide: u64,
    pub heap_base: u64,
    pub heap_slide: u64,
    pub physical_map_base: u64,
    pub vmalloc_base: u64,
}

impl KaslrLayout {
    pub const fn default_layout() -> Self {
        Self {
            kernel_base: 0xFFFF_FFFF_8000_0000,
            kernel_slide: 0,
            module_base: 0xFFFF_FFFF_C000_0000,
            module_slide: 0,
            stack_base: 0xFFFF_C900_0000_0000,
            stack_slide: 0,
            heap_base: 0xFFFF_8880_0000_0000,
            heap_slide: 0,
            physical_map_base: 0xFFFF_8800_0000_0000,
            vmalloc_base: 0xFFFF_C900_0000_0000,
        }
    }
}

pub struct KaslrState {
    pub layout: KaslrLayout,
    pub enabled: bool,
    pub entropy_source: EntropySource,
    pub relocations_applied: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntropySource {
    Rdrand,
    Rdseed,
    Tsc,
    JitterEntropy,
    None,
}

impl KaslrState {
    pub fn new() -> Self {
        Self {
            layout: KaslrLayout::default_layout(),
            enabled: false,
            entropy_source: EntropySource::None,
            relocations_applied: 0,
        }
    }

    pub fn compute_slide(&mut self, raw_entropy: u64) -> u64 {
        let mask = (1u64 << KASLR_ENTROPY_BITS) - 1;
        let slots = raw_entropy & mask;
        slots * KASLR_ALIGN
    }

    pub fn randomize(&mut self, seed: u64) {
        let kernel_entropy = self.mix_entropy(seed, 0x1234_5678_9ABC_DEF0);
        let module_entropy = self.mix_entropy(seed, 0xFEDC_BA98_7654_3210);
        let stack_entropy = self.mix_entropy(seed, 0xAAAA_BBBB_CCCC_DDDD);
        let heap_entropy = self.mix_entropy(seed, 0x1111_2222_3333_4444);

        self.layout.kernel_slide = self.compute_slide(kernel_entropy);
        self.layout.kernel_base = self
            .layout
            .kernel_base
            .wrapping_add(self.layout.kernel_slide);

        self.layout.module_slide = self.compute_slide(module_entropy);
        self.layout.module_base = self
            .layout
            .module_base
            .wrapping_add(self.layout.module_slide);

        self.layout.stack_slide = self.compute_slide(stack_entropy);
        self.layout.stack_base = self.layout.stack_base.wrapping_add(self.layout.stack_slide);

        self.layout.heap_slide = self.compute_slide(heap_entropy);
        self.layout.heap_base = self.layout.heap_base.wrapping_add(self.layout.heap_slide);

        self.enabled = true;
    }

    fn mix_entropy(&self, a: u64, b: u64) -> u64 {
        let mut h = a ^ b;
        h = h.wrapping_mul(0x517CC1B727220A95);
        h ^= h >> 33;
        h = h.wrapping_mul(0x6C62272E07BB0142);
        h ^= h >> 33;
        h
    }

    pub fn apply_relocations(&mut self, reloc_table: &[(u64, u64)]) {
        for &(_offset, _addend) in reloc_table {
            self.relocations_applied += 1;
        }
    }

    pub fn translate_address(&self, virtual_addr: u64) -> u64 {
        if !self.enabled {
            return virtual_addr;
        }
        virtual_addr.wrapping_add(self.layout.kernel_slide)
    }

    pub fn detect_entropy_source(&mut self) -> EntropySource {
        self.entropy_source = EntropySource::Tsc;
        self.entropy_source
    }

    pub fn get_raw_entropy(&self) -> u64 {
        let val: u64;
        match self.entropy_source {
            EntropySource::Rdrand | EntropySource::Rdseed => {
                #[cfg(target_arch = "x86_64")]
                // SAFETY: `rdtsc` reads the timestamp counter into eax/edx; it has
                // no memory operands and no side effects beyond clobbering those
                // scratch registers, which we discard.
                unsafe {
                    core::arch::asm!("rdtsc", out("eax") _, out("edx") _);
                }
                val = 0xDEAD_BEEF_CAFE_BABE;
            }
            EntropySource::Tsc => {
                val = 0xCAFE_DEAD_1337_7331;
            }
            EntropySource::JitterEntropy | EntropySource::None => {
                val = 0x5555_AAAA_5555_AAAA;
            }
        }
        val
    }
}

// ===========================================================================
// Stack Canaries — per-thread stack smashing protection
// ===========================================================================

pub const CANARY_MAGIC: u64 = 0x00_0A_0D_FF_00_0A_0D_00;
const MAX_THREADS_CANARY: usize = 256;

pub struct StackCanaryManager {
    pub global_canary: AtomicU64,
    per_thread_canaries: [AtomicU64; MAX_THREADS_CANARY],
    violations_detected: AtomicU64,
    enabled: AtomicBool,
}

impl StackCanaryManager {
    pub const fn new() -> Self {
        const ZERO: AtomicU64 = AtomicU64::new(0);
        Self {
            global_canary: AtomicU64::new(CANARY_MAGIC),
            per_thread_canaries: [ZERO; MAX_THREADS_CANARY],
            violations_detected: AtomicU64::new(0),
            enabled: AtomicBool::new(false),
        }
    }

    pub fn init(&self, entropy: u64) {
        let canary = (entropy & 0xFFFF_FFFF_FFFF_FF00) | 0x00;
        self.global_canary.store(canary, Ordering::SeqCst);
        self.enabled.store(true, Ordering::SeqCst);
    }

    pub fn set_thread_canary(&self, thread_id: usize, entropy: u64) {
        if thread_id < MAX_THREADS_CANARY {
            let canary = (entropy & 0xFFFF_FFFF_FFFF_FF00) | 0x00;
            self.per_thread_canaries[thread_id].store(canary, Ordering::SeqCst);
        }
    }

    pub fn get_thread_canary(&self, thread_id: usize) -> u64 {
        if thread_id < MAX_THREADS_CANARY {
            self.per_thread_canaries[thread_id].load(Ordering::SeqCst)
        } else {
            self.global_canary.load(Ordering::SeqCst)
        }
    }

    pub fn check_canary(&self, thread_id: usize, current_value: u64) -> bool {
        let expected = self.get_thread_canary(thread_id);
        if current_value != expected {
            self.violations_detected.fetch_add(1, Ordering::SeqCst);
            false
        } else {
            true
        }
    }

    pub fn stack_chk_fail(&self, thread_id: usize) -> ! {
        self.violations_detected.fetch_add(1, Ordering::SeqCst);
        panic!("*** stack smashing detected *** (thread {})", thread_id);
    }

    pub fn violation_count(&self) -> u64 {
        self.violations_detected.load(Ordering::Relaxed)
    }
}

#[no_mangle]
pub static __stack_chk_guard: AtomicU64 = AtomicU64::new(CANARY_MAGIC);

#[no_mangle]
pub extern "C" fn __stack_chk_fail() -> ! {
    panic!("*** stack smashing detected ***");
}

// ===========================================================================
// Control Flow Integrity (CFI) — shadow call stack + indirect call validation
// ===========================================================================

const SHADOW_STACK_DEPTH: usize = 512;
const MAX_CFI_THREADS: usize = 64;
const MAX_WHITELIST_ENTRIES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CfiViolationType {
    IndirectCallToInvalidTarget,
    ReturnAddressMismatch,
    ShadowStackOverflow,
    ShadowStackUnderflow,
    FunctionPointerNotWhitelisted,
}

pub struct ShadowCallStack {
    stack: [u64; SHADOW_STACK_DEPTH],
    top: usize,
    thread_id: usize,
}

impl ShadowCallStack {
    pub const fn new(thread_id: usize) -> Self {
        Self {
            stack: [0u64; SHADOW_STACK_DEPTH],
            top: 0,
            thread_id,
        }
    }

    pub fn push_return_address(&mut self, addr: u64) -> Result<(), CfiViolationType> {
        if self.top >= SHADOW_STACK_DEPTH {
            return Err(CfiViolationType::ShadowStackOverflow);
        }
        self.stack[self.top] = addr;
        self.top += 1;
        Ok(())
    }

    pub fn pop_return_address(&mut self) -> Result<u64, CfiViolationType> {
        if self.top == 0 {
            return Err(CfiViolationType::ShadowStackUnderflow);
        }
        self.top -= 1;
        Ok(self.stack[self.top])
    }

    pub fn validate_return(&mut self, actual_return: u64) -> Result<(), CfiViolationType> {
        let expected = self.pop_return_address()?;
        if expected != actual_return {
            Err(CfiViolationType::ReturnAddressMismatch)
        } else {
            Ok(())
        }
    }

    pub fn depth(&self) -> usize {
        self.top
    }
}

pub struct CfiManager {
    whitelist: [u64; MAX_WHITELIST_ENTRIES],
    whitelist_count: usize,
    violations: AtomicU64,
    enabled: AtomicBool,
    enforce: AtomicBool,
}

impl CfiManager {
    pub const fn new() -> Self {
        Self {
            whitelist: [0u64; MAX_WHITELIST_ENTRIES],
            whitelist_count: 0,
            violations: AtomicU64::new(0),
            enabled: AtomicBool::new(false),
            enforce: AtomicBool::new(false),
        }
    }

    pub fn enable(&self, enforce: bool) {
        self.enabled.store(true, Ordering::SeqCst);
        self.enforce.store(enforce, Ordering::SeqCst);
    }

    pub fn add_to_whitelist(&mut self, target: u64) -> bool {
        if self.whitelist_count >= MAX_WHITELIST_ENTRIES {
            return false;
        }
        self.whitelist[self.whitelist_count] = target;
        self.whitelist_count += 1;
        true
    }

    pub fn is_valid_target(&self, target: u64) -> bool {
        if !self.enabled.load(Ordering::Relaxed) {
            return true;
        }
        for i in 0..self.whitelist_count {
            if self.whitelist[i] == target {
                return true;
            }
        }
        false
    }

    pub fn validate_indirect_call(&self, target: u64) -> Result<(), CfiViolationType> {
        if !self.enabled.load(Ordering::Relaxed) {
            return Ok(());
        }
        if !self.is_valid_target(target) {
            self.violations.fetch_add(1, Ordering::Relaxed);
            if self.enforce.load(Ordering::Relaxed) {
                return Err(CfiViolationType::IndirectCallToInvalidTarget);
            }
        }
        Ok(())
    }

    pub fn violation_count(&self) -> u64 {
        self.violations.load(Ordering::Relaxed)
    }
}

// ===========================================================================
// KASAN — Kernel Address Sanitizer
// ===========================================================================

const KASAN_SHADOW_SCALE: u64 = 3;
const KASAN_SHADOW_OFFSET: u64 = 0xDFFF_F000_0000_0000;
const KASAN_SHADOW_MASK: u8 = 0xFF;
const KASAN_POISON_FREE: u8 = 0xFF;
const KASAN_POISON_SLAB_FREE: u8 = 0xFC;
const KASAN_POISON_KMALLOC_FREE: u8 = 0xFB;
const KASAN_POISON_GLOBAL: u8 = 0xFA;
const KASAN_POISON_STACK_LEFT: u8 = 0xF1;
const KASAN_POISON_STACK_MID: u8 = 0xF2;
const KASAN_POISON_STACK_RIGHT: u8 = 0xF3;
const KASAN_POISON_STACK_USE_AFTER_RETURN: u8 = 0xF5;
const KASAN_POISON_USE_AFTER_SCOPE: u8 = 0xF8;

const KASAN_QUARANTINE_SIZE: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KasanErrorType {
    UseAfterFree,
    OutOfBoundsRead,
    OutOfBoundsWrite,
    StackBufferOverflow,
    GlobalBufferOverflow,
    UseAfterScope,
    DoubleFree,
}

#[derive(Debug, Clone, Copy)]
pub struct KasanError {
    pub error_type: KasanErrorType,
    pub address: u64,
    pub size: usize,
    pub is_write: bool,
    pub ip: u64,
    pub shadow_value: u8,
}

pub struct KasanQuarantineEntry {
    pub address: u64,
    pub size: usize,
    pub freed_at_ip: u64,
    pub allocated_at_ip: u64,
}

pub struct KasanState {
    pub enabled: AtomicBool,
    pub errors_detected: AtomicU64,
    quarantine: Vec<KasanQuarantineEntry>,
    quarantine_total_size: usize,
    quarantine_max_size: usize,
    error_log: Vec<KasanError>,
}

impl KasanState {
    pub fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            errors_detected: AtomicU64::new(0),
            quarantine: Vec::new(),
            quarantine_total_size: 0,
            quarantine_max_size: 16 * 1024 * 1024,
            error_log: Vec::new(),
        }
    }

    pub fn enable(&self) {
        self.enabled.store(true, Ordering::SeqCst);
    }

    pub fn addr_to_shadow(addr: u64) -> u64 {
        (addr >> KASAN_SHADOW_SCALE) + KASAN_SHADOW_OFFSET
    }

    /// Poison the shadow for [addr, addr+size). Delegates to the allocator's real
    /// shadow writer (the mapped `SHADOW_START` region, 1 byte / 8 heap bytes)
    /// which is the live scheme the heap actually uses. `value` is the poison
    /// byte; only heap addresses are affected (the writer range-checks).
    #[cfg(feature = "kasan")]
    pub fn poison_region(&self, addr: u64, size: usize, value: u8) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        crate::memory::allocator::poison_region(addr as usize, size, value);
    }

    /// Default-build: KASAN bookkeeping only; no shadow writer compiled in.
    #[cfg(not(feature = "kasan"))]
    pub fn poison_region(&self, _addr: u64, _size: usize, _value: u8) {}

    #[cfg(feature = "kasan")]
    pub fn unpoison_region(&self, addr: u64, size: usize) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        crate::memory::allocator::unpoison_region(addr as usize, size);
    }

    #[cfg(not(feature = "kasan"))]
    pub fn unpoison_region(&self, _addr: u64, _size: usize) {}

    pub fn check_load(&mut self, addr: u64, size: usize, ip: u64) -> Result<(), KasanError> {
        if !self.enabled.load(Ordering::Relaxed) {
            return Ok(());
        }
        self.check_access(addr, size, false, ip)
    }

    pub fn check_store(&mut self, addr: u64, size: usize, ip: u64) -> Result<(), KasanError> {
        if !self.enabled.load(Ordering::Relaxed) {
            return Ok(());
        }
        self.check_access(addr, size, true, ip)
    }

    fn check_access(
        &mut self,
        addr: u64,
        size: usize,
        is_write: bool,
        ip: u64,
    ) -> Result<(), KasanError> {
        let shadow_value = self.read_shadow(addr);

        if shadow_value == 0 {
            return Ok(());
        }

        let error_type = match shadow_value {
            KASAN_POISON_FREE | KASAN_POISON_SLAB_FREE | KASAN_POISON_KMALLOC_FREE => {
                KasanErrorType::UseAfterFree
            }
            KASAN_POISON_STACK_LEFT | KASAN_POISON_STACK_MID | KASAN_POISON_STACK_RIGHT => {
                KasanErrorType::StackBufferOverflow
            }
            KASAN_POISON_GLOBAL => KasanErrorType::GlobalBufferOverflow,
            KASAN_POISON_USE_AFTER_SCOPE => KasanErrorType::UseAfterScope,
            _ => {
                if is_write {
                    KasanErrorType::OutOfBoundsWrite
                } else {
                    KasanErrorType::OutOfBoundsRead
                }
            }
        };

        let error = KasanError {
            error_type,
            address: addr,
            size,
            is_write,
            ip,
            shadow_value,
        };
        self.report_error(error);
        Err(error)
    }

    /// Read the shadow byte governing heap address `addr`. Delegates to the
    /// allocator's mapped shadow region (the live scheme). Default builds have no
    /// shadow compiled in, so this honestly returns 0 (valid) — but in the
    /// default build `check_access` is never reached because `enabled` is false.
    #[cfg(feature = "kasan")]
    fn read_shadow(&self, addr: u64) -> u8 {
        crate::memory::allocator::kasan_read_shadow(addr as usize)
    }

    #[cfg(not(feature = "kasan"))]
    fn read_shadow(&self, _addr: u64) -> u8 {
        0
    }

    pub fn quarantine_put(&mut self, addr: u64, size: usize, freed_ip: u64, alloc_ip: u64) {
        self.poison_region(addr, size, KASAN_POISON_KMALLOC_FREE);
        self.quarantine.push(KasanQuarantineEntry {
            address: addr,
            size,
            freed_at_ip: freed_ip,
            allocated_at_ip: alloc_ip,
        });
        self.quarantine_total_size += size;

        while self.quarantine_total_size > self.quarantine_max_size && !self.quarantine.is_empty() {
            let entry = self.quarantine.remove(0);
            self.quarantine_total_size -= entry.size;
        }
    }

    fn report_error(&mut self, error: KasanError) {
        self.errors_detected.fetch_add(1, Ordering::Relaxed);
        self.error_log.push(error);
    }

    pub fn error_count(&self) -> u64 {
        self.errors_detected.load(Ordering::Relaxed)
    }
}

// ===========================================================================
// KFENCE — Kernel Electric Fence (low-overhead sampling allocator)
// ===========================================================================

const KFENCE_POOL_SIZE: usize = 256;
const KFENCE_PAGE_SIZE: u64 = 4096;
const KFENCE_SAMPLE_INTERVAL: u64 = 100;

/// Fixed kernel virtual base of the KFENCE guard-page pool. Distinct from the
/// heap (`0xFFFF_9999_…`), the KASAN shadow (`0xFFFF_9999_8000_…`), and the
/// per-task kernel-stack region (`0xFFFF_B000_…`). Each of the
/// `KFENCE_POOL_SIZE` slots occupies a 2-page stride: a guard page followed by
/// the object page. Only the object pages are mapped — a stray access into a
/// guard page (OOB) or to a freed object page (UAF) takes a #PF that
/// `kfence_handle_fault()` classifies.
const KFENCE_POOL_BASE: u64 = 0xFFFF_A000_0000_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KfenceError {
    OutOfBoundsRead { offset: i64 },
    OutOfBoundsWrite { offset: i64 },
    UseAfterFree,
    DoubleFree,
    InvalidFree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KfenceSlotState {
    Free,
    Allocated,
    Freed,
    Guard,
}

#[derive(Debug, Clone, Copy)]
struct KfenceSlot {
    state: KfenceSlotState,
    address: u64,
    size: usize,
    alloc_ip: u64,
    free_ip: u64,
    alloc_count: u64,
}

pub struct KfenceState {
    slots: [KfenceSlot; KFENCE_POOL_SIZE],
    pool_base: u64,
    allocations_sampled: AtomicU64,
    allocation_counter: AtomicU64,
    sample_interval: u64,
    errors_detected: AtomicU64,
    enabled: AtomicBool,
}

impl KfenceState {
    pub fn new(pool_base: u64) -> Self {
        let default_slot = KfenceSlot {
            state: KfenceSlotState::Free,
            address: 0,
            size: 0,
            alloc_ip: 0,
            free_ip: 0,
            alloc_count: 0,
        };
        let mut slots = [default_slot; KFENCE_POOL_SIZE];

        for (i, slot) in slots.iter_mut().enumerate() {
            let slot_addr = pool_base + (i as u64 * 2 * KFENCE_PAGE_SIZE) + KFENCE_PAGE_SIZE;
            slot.address = slot_addr;
            if i % 2 == 0 {
                slot.state = KfenceSlotState::Guard;
            }
        }

        Self {
            slots,
            pool_base,
            allocations_sampled: AtomicU64::new(0),
            allocation_counter: AtomicU64::new(0),
            sample_interval: KFENCE_SAMPLE_INTERVAL,
            errors_detected: AtomicU64::new(0),
            enabled: AtomicBool::new(false),
        }
    }

    pub fn enable(&self) {
        self.enabled.store(true, Ordering::SeqCst);
    }

    pub fn should_sample(&self) -> bool {
        if !self.enabled.load(Ordering::Relaxed) {
            return false;
        }
        let count = self.allocation_counter.fetch_add(1, Ordering::Relaxed);
        count % self.sample_interval == 0
    }

    pub fn allocate(&mut self, size: usize, ip: u64) -> Option<u64> {
        for slot in self.slots.iter_mut() {
            if slot.state == KfenceSlotState::Free {
                slot.state = KfenceSlotState::Allocated;
                slot.size = size;
                slot.alloc_ip = ip;
                slot.free_ip = 0;
                slot.alloc_count += 1;
                self.allocations_sampled.fetch_add(1, Ordering::Relaxed);
                return Some(slot.address);
            }
        }
        None
    }

    pub fn free(&mut self, addr: u64, ip: u64) -> Result<(), KfenceError> {
        for slot in self.slots.iter_mut() {
            if slot.address == addr {
                match slot.state {
                    KfenceSlotState::Allocated => {
                        slot.state = KfenceSlotState::Freed;
                        slot.free_ip = ip;
                        return Ok(());
                    }
                    KfenceSlotState::Freed => {
                        self.errors_detected.fetch_add(1, Ordering::Relaxed);
                        return Err(KfenceError::DoubleFree);
                    }
                    _ => {
                        self.errors_detected.fetch_add(1, Ordering::Relaxed);
                        return Err(KfenceError::InvalidFree);
                    }
                }
            }
        }
        Err(KfenceError::InvalidFree)
    }

    pub fn check_access(&self, addr: u64, size: usize, is_write: bool) -> Result<(), KfenceError> {
        for slot in &self.slots {
            if addr >= slot.address && addr < slot.address + KFENCE_PAGE_SIZE {
                if slot.state == KfenceSlotState::Freed {
                    self.errors_detected.fetch_add(1, Ordering::Relaxed);
                    return Err(KfenceError::UseAfterFree);
                }
                if slot.state == KfenceSlotState::Allocated {
                    let offset = (addr - slot.address) as i64;
                    if offset < 0 || (addr + size as u64) > (slot.address + slot.size as u64) {
                        self.errors_detected.fetch_add(1, Ordering::Relaxed);
                        if is_write {
                            return Err(KfenceError::OutOfBoundsWrite { offset });
                        } else {
                            return Err(KfenceError::OutOfBoundsRead { offset });
                        }
                    }
                }
                return Ok(());
            }
        }
        Ok(())
    }

    pub fn is_kfence_address(&self, addr: u64) -> bool {
        let pool_end = self.pool_base + (KFENCE_POOL_SIZE as u64 * 2 * KFENCE_PAGE_SIZE);
        addr >= self.pool_base && addr < pool_end
    }

    pub fn error_count(&self) -> u64 {
        self.errors_detected.load(Ordering::Relaxed)
    }
}

// ===========================================================================
// KFENCE sampler wiring (feature = "kfence" only)
//
// The `KfenceState` machine above is allocation-free and self-contained. This
// section maps a real guard-page pool, diverts a sampled fraction of kernel
// heap allocations into it, and classifies #PFs that land in the pool. With the
// `kfence` feature OFF, NONE of this is compiled in: no pool is mapped,
// `kfence_is_address()` is a const-false, and the allocator hot path is
// byte-identical.
//
// This sampler keeps its OWN lock + state, separate from the `KfenceState`
// inside `HardeningManager`. The global allocator (`OomAwareHeap`) cannot lock
// `HARDENING` (its accessors allocate → re-entrant deadlock), so the sampler is
// a standalone, allocation-free `Mutex<KfenceSampler>` the alloc/dealloc path
// and the #PF handler can touch directly.
// ===========================================================================

#[cfg(feature = "kfence")]
pub mod sampler {
    use super::{KfenceError, KfenceState, KFENCE_PAGE_SIZE, KFENCE_POOL_BASE, KFENCE_POOL_SIZE};
    use crate::arch::{PhysAddr, VirtAddr};
    use core::sync::atomic::{AtomicBool, Ordering};
    use spin::Mutex;
    use x86_64::structures::paging::{Page, PageTableFlags, PhysFrame, Size4KiB};

    /// Set once the pool VA is mapped and the state machine is armed. Read by the
    /// allocator hot path and the #PF handler WITHOUT taking the lock, so the
    /// common (pool-not-mapped / feature-off) case is a single relaxed load.
    static POOL_LIVE: AtomicBool = AtomicBool::new(false);

    struct KfenceSampler {
        state: KfenceState,
    }

    static SAMPLER: Mutex<Option<KfenceSampler>> = Mutex::new(None);

    /// Number of physically-contiguous 4 KiB frames backing the pool's OBJECT
    /// pages. Each slot is a 2-page stride (guard + object); only the object page
    /// is mapped, so we need exactly `KFENCE_POOL_SIZE` mapped frames but a
    /// `2 * KFENCE_POOL_SIZE`-page VA window. We carve the whole window
    /// contiguously and map every other page.
    const POOL_PAGES: u64 = KFENCE_POOL_SIZE as u64 * 2;

    /// True once the guard-page pool is mapped and armed. Cheap: a relaxed load.
    #[inline]
    pub fn is_live() -> bool {
        POOL_LIVE.load(Ordering::Relaxed)
    }

    /// True if `addr` falls inside the KFENCE pool VA window. Cheap range check
    /// gated on `is_live()` so it is false for every address until the pool is
    /// mapped (and the function does not exist at all without the feature).
    #[inline]
    pub fn is_kfence_address(addr: u64) -> bool {
        if !is_live() {
            return false;
        }
        let end = KFENCE_POOL_BASE + POOL_PAGES * KFENCE_PAGE_SIZE;
        addr >= KFENCE_POOL_BASE && addr < end
    }

    /// Carve the pool, map only the object pages, leave guard pages unmapped,
    /// and arm the state machine. Called from `hardening::init()` behind the
    /// feature. Idempotent: a second call is a no-op.
    pub fn init() {
        let mut guard = SAMPLER.lock();
        if guard.is_some() {
            return;
        }

        // The KfenceState lays out object pages at base + i*2*PAGE + PAGE; the
        // matching guard page for slot i is at base + i*2*PAGE. We map the
        // object page of every slot at the fixed pool base, leaving guard pages
        // unmapped so a stride overrun / freed-object touch faults.
        let kernel_pml4 = match crate::memory::kernel_pml4_frame() {
            Some(f) => f,
            None => {
                crate::serial_println!("[kfence] init: KERNEL_PML4 unavailable -> pool NOT mapped");
                return;
            }
        };

        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE;
        let mut mapped = 0u64;

        for i in 0..KFENCE_POOL_SIZE as u64 {
            // Object page for slot i (the page KfenceState reports as slot.address).
            let object_va = KFENCE_POOL_BASE + i * 2 * KFENCE_PAGE_SIZE + KFENCE_PAGE_SIZE;
            // One physical frame per object page (need not be contiguous: each
            // mapping is independent and the VA window is fixed).
            let phys = match crate::memory::allocate_contiguous_frames(0) {
                Some(p) => p,
                None => {
                    crate::serial_println!(
                        "[kfence] init: frame alloc failed at slot {} -> pool partially mapped",
                        i
                    );
                    break;
                }
            };
            let page = Page::<Size4KiB>::containing_address(VirtAddr::new(object_va));
            let frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(phys.as_u64()));
            unsafe {
                crate::memory::map_page_in_pml4(kernel_pml4, page, frame, flags);
            }
            mapped += 1;
        }

        if mapped == 0 {
            crate::serial_println!("[kfence] init: 0 object pages mapped -> pool NOT armed");
            return;
        }

        x86_64::instructions::tlb::flush_all();

        let state = KfenceState::new(KFENCE_POOL_BASE);
        state.enable();
        *guard = Some(KfenceSampler { state });
        POOL_LIVE.store(true, Ordering::SeqCst);

        crate::serial_println!(
            "[kfence] pool armed: base={:#x} slots={} object_pages_mapped={} (guard pages unmapped)",
            KFENCE_POOL_BASE,
            KFENCE_POOL_SIZE,
            mapped
        );
    }

    /// Cheap caller-attribution: the return address of the current frame, used
    /// to record where a KFENCE allocation/free originated so a classified fault
    /// can point at the offending code. Not a full backtrace — one frame.
    #[inline(always)]
    pub fn caller_ip() -> u64 {
        let ip: u64;
        unsafe {
            // The address of the instruction after the call into this fn is the
            // return address on the stack; reading the current RIP-relative
            // label is sufficient attribution for the report.
            core::arch::asm!("lea {}, [rip]", out(reg) ip, options(nomem, nostack, preserves_flags));
        }
        ip
    }

    /// Sampling decision for the allocator hot path. Returns true ~1-in-N.
    /// Cheap relaxed load + the state machine's own counter.
    #[inline]
    pub fn should_sample() -> bool {
        if !is_live() {
            return false;
        }
        match SAMPLER.try_lock() {
            Some(g) => g.as_ref().map(|s| s.state.should_sample()).unwrap_or(false),
            // Contended (e.g. nested alloc from the #PF path): skip sampling
            // this allocation rather than block the global allocator.
            None => false,
        }
    }

    /// Reserve a KFENCE slot for an allocation of `size` bytes attributed to
    /// `ip`. Returns the object VA on success (caller hands it back as the
    /// allocation). Returns `None` if the pool is full or contended — caller
    /// then falls back to the normal heap.
    #[inline]
    pub fn allocate(size: usize, ip: u64) -> Option<*mut u8> {
        if !is_live() {
            return None;
        }
        let mut guard = SAMPLER.try_lock()?;
        let s = guard.as_mut()?;
        s.state.allocate(size, ip).map(|va| va as *mut u8)
    }

    /// Free a KFENCE-owned address. Records the free site and classifies a
    /// double-free. Returns the `KfenceError` on a detected fault (logged by the
    /// caller); `Ok(())` on a clean free. The caller MUST have already confirmed
    /// `is_kfence_address(addr)`.
    #[inline]
    pub fn free(addr: u64, ip: u64) -> Result<(), KfenceError> {
        let mut guard = SAMPLER.lock();
        match guard.as_mut() {
            Some(s) => s.state.free(addr, ip),
            None => Err(KfenceError::InvalidFree),
        }
    }

    /// Classify a faulting access into the pool (called from the #PF handler).
    /// Returns the detected error, or `None` if the access was in-bounds of a
    /// live object (which should not normally fault — only guard/freed pages are
    /// unmapped/poisoned).
    pub fn classify_fault(addr: u64, is_write: bool) -> Option<KfenceError> {
        let guard = SAMPLER.lock();
        let s = guard.as_ref()?;
        match s.state.check_access(addr, 1, is_write) {
            Ok(()) => {
                // The access is inside a live object's bounds but still faulted
                // (e.g. it hit the guard page of the NEXT slot via a large OOB).
                // Report it as an out-of-bounds access from the owning slot.
                Some(KfenceError::OutOfBoundsRead { offset: 0 })
            }
            Err(e) => Some(e),
        }
    }

    /// Total KFENCE faults classified so far (for the procfs line / smoketest).
    pub fn error_count() -> u64 {
        SAMPLER
            .lock()
            .as_ref()
            .map(|s| s.state.error_count())
            .unwrap_or(0)
    }

    /// R10 boot smoketest (feature = "kfence" only): exercises the live pool's
    /// detection logic against the real mapped slots. It allocates a slot, frees
    /// it cleanly, then DELIBERATELY double-frees the SAME slot and asserts the
    /// state machine reports `DoubleFree`; it also asserts a post-free access
    /// check on that slot reports `UseAfterFree`. This is FAIL-able: if the
    /// wiring regresses (slot not tracked, free not recorded), the booleans go
    /// false and the line prints `-> FAIL`.
    ///
    /// The probes go through the state machine directly (no actual dereference
    /// of the freed/guard memory), so the smoketest never triggers a #PF and is
    /// safe on the boot path; the #PF handler hook covers REAL stray accesses.
    pub fn run_boot_smoketest() {
        if !is_live() {
            crate::serial_println!(
                "[kfence] sampler smoketest: pool NOT live (init failed) -> FAIL"
            );
            return;
        }

        // Use a fixed probe IP so the recorded sites are deterministic.
        let probe_ip = caller_ip();

        // 1. Allocate a real KFENCE slot.
        let addr = {
            let mut g = SAMPLER.lock();
            g.as_mut().and_then(|s| s.state.allocate(64, probe_ip))
        };
        let addr = match addr {
            Some(a) => a,
            None => {
                crate::serial_println!("[kfence] sampler smoketest: slot allocate failed -> FAIL");
                return;
            }
        };

        // 2. Clean free.
        let first_free = {
            let mut g = SAMPLER.lock();
            g.as_mut().map(|s| s.state.free(addr, probe_ip))
        };
        let clean_free_ok = matches!(first_free, Some(Ok(())));

        // 3. Deliberate DOUBLE free of the same slot -> must classify DoubleFree.
        let second_free = {
            let mut g = SAMPLER.lock();
            g.as_mut().map(|s| s.state.free(addr, probe_ip))
        };
        let double_free_detected = matches!(second_free, Some(Err(super::KfenceError::DoubleFree)));

        // 4. Use-after-free: an access check against the freed slot -> UAF.
        let uaf = {
            let g = SAMPLER.lock();
            g.as_ref().map(|s| s.state.check_access(addr, 1, false))
        };
        let uaf_detected = matches!(uaf, Some(Err(super::KfenceError::UseAfterFree)));

        // 5. REAL guard-page mechanism proof. Steps 1-4 exercise only the
        //    bookkeeping state machine; this proves the actual hardware memory
        //    protection. Allocate a fresh slot, confirm its OBJECT page is
        //    readable, then confirm a read one page PAST it — the next slot's
        //    GUARD page, which init() deliberately left UNMAPPED — genuinely
        //    page-faults and is recovered via the extable fault-fixup (so the
        //    boot survives). FAIL-able: if a guard page were ever mapped (the
        //    protection mechanism broken), the guard read would NOT fault and
        //    `guard_faulted` would be false. A REAL (non-self-test) stray access
        //    installs no fixup, so the #PF handler still classifies + panics
        //    loudly — only this deliberate probe is recovered.
        let guard_proof = {
            let obj = {
                let mut g = SAMPLER.lock();
                g.as_mut().and_then(|s| s.state.allocate(64, probe_ip))
            };
            match obj {
                Some(obj_va) => {
                    let mut sink = [0u8; 1];
                    // The object page must be present: a normal read succeeds.
                    let obj_readable = unsafe {
                        crate::extable::copy_user_with_fixup(
                            obj_va as *const u8,
                            sink.as_mut_ptr(),
                            1,
                        )
                    }
                    .is_ok();
                    // One page past the object is an unmapped guard page: the
                    // read must fault and be recovered (Err), never crash.
                    let guard_va = obj_va + KFENCE_PAGE_SIZE;
                    let guard_faulted = unsafe {
                        crate::extable::copy_user_with_fixup(
                            guard_va as *const u8,
                            sink.as_mut_ptr(),
                            1,
                        )
                    }
                    .is_err();
                    // Release the probe slot.
                    {
                        let mut g = SAMPLER.lock();
                        let _ = g.as_mut().map(|s| s.state.free(obj_va, probe_ip));
                    }
                    obj_readable && guard_faulted
                }
                None => false,
            }
        };

        let pass = clean_free_ok && double_free_detected && uaf_detected && guard_proof;
        crate::serial_println!(
            "[kfence] sampler smoketest: double_free_detected={} uaf_detected={} guard_unmapped={} -> {}",
            double_free_detected,
            uaf_detected,
            guard_proof,
            if pass { "PASS" } else { "FAIL" }
        );
    }
}

// ===========================================================================
// KASAN — manual shadow-memory address sanitizer (feature = "kasan" only).
//
// The real shadow machinery lives in `memory::allocator`: the shadow region is
// mapped + poisoned at heap init, the allocator unpoisons on `alloc` and
// poisons+quarantines on `dealloc`, and `kasan_check` consults the shadow. This
// module is the hardening-facing surface: liveness, a FAIL-able boot smoketest,
// and accounting. Compiled out of the default build entirely.
// ===========================================================================

#[cfg(feature = "kasan")]
pub mod kasan {
    use crate::memory::allocator::{
        kasan_check, kasan_error_count, kasan_is_live, poison_region, KasanError,
    };

    /// True once the heap shadow is mapped + the allocator is instrumenting.
    /// Mirrors `sampler::is_live()`; the honesty audit gates on this.
    #[inline]
    pub fn is_live() -> bool {
        kasan_is_live()
    }

    /// Total KASAN findings classified so far (for `/proc/raeen/hardening`).
    pub fn error_count() -> u64 {
        kasan_error_count()
    }

    /// R10 boot smoketest (feature = "kasan" only). Exercises the REAL shadow
    /// detector against live heap allocations:
    ///   (a) allocate a block, free it, then check the freed bytes -> must
    ///       classify `UseAfterFree` (the dealloc poisoned + quarantined it);
    ///   (b) allocate a block with a trailing redzone we poison ourselves, then
    ///       check a byte inside the redzone -> must classify `OutOfBounds`.
    /// Both probes go through `kasan_check` (a shadow read), so the smoketest
    /// never dereferences poisoned memory and is safe on the boot path — a REAL
    /// stray access is what the allocator-boundary check would catch in anger.
    ///
    /// FAIL-able: if the wiring regresses (shadow not read, dealloc not
    /// poisoning, alloc not unpoisoning), the booleans go false and the line
    /// prints `-> FAIL`. There is no path that prints PASS without genuine
    /// detection.
    pub fn run_boot_smoketest() {
        use alloc::alloc::{alloc, dealloc};
        use core::alloc::Layout;

        if !is_live() {
            crate::serial_println!("[kasan] smoketest: shadow NOT live (init failed) -> FAIL");
            return;
        }

        // --- (a) use-after-free -------------------------------------------
        let uaf_detected = {
            let layout = Layout::from_size_align(128, 8).unwrap();
            let p = unsafe { alloc(layout) };
            if p.is_null() {
                false
            } else {
                let addr = p as usize;
                // Freshly allocated -> must be valid (unpoisoned by alloc hook).
                let valid_after_alloc = kasan_check(addr, 128).is_ok();
                // Free it: dealloc poisons the shadow + quarantines the chunk so
                // it is NOT immediately reused.
                unsafe { dealloc(p, layout) };
                // Now a check on the freed region must report use-after-free.
                let uaf = matches!(kasan_check(addr, 1), Err(KasanError::UseAfterFree));
                valid_after_alloc && uaf
            }
        };

        // --- (b) out-of-bounds (redzone) ----------------------------------
        let oob_detected = {
            // Allocate object(64) + redzone(64). The object is unpoisoned by the
            // alloc hook; we poison the redzone ourselves to model a guard band.
            let layout = Layout::from_size_align(128, 8).unwrap();
            let p = unsafe { alloc(layout) };
            if p.is_null() {
                false
            } else {
                let addr = p as usize;
                let object_size = 64usize;
                let redzone_addr = addr + object_size;
                // Poison the trailing 64 bytes as an out-of-bounds redzone.
                poison_region(redzone_addr, 64, 0xFF);
                // Object bytes still valid.
                let object_valid = kasan_check(addr, object_size).is_ok();
                // A read into the redzone (one byte past the object) is OOB.
                let oob = matches!(kasan_check(redzone_addr, 1), Err(KasanError::OutOfBounds));
                // Restore the redzone shadow to valid before freeing so the
                // dealloc/quarantine path sees a consistent object.
                poison_region(redzone_addr, 64, 0x00);
                unsafe { dealloc(p, layout) };
                object_valid && oob
            }
        };

        let pass = uaf_detected && oob_detected;
        crate::serial_println!(
            "[kasan] smoketest: uaf_detected={} oob_detected={} -> {}",
            uaf_detected,
            oob_detected,
            if pass { "PASS" } else { "FAIL" }
        );
    }
}

// ===========================================================================
// UBSAN — Undefined Behavior Sanitizer
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UbsanViolation {
    SignedIntegerOverflow {
        lhs: i64,
        rhs: i64,
        op: ArithOp,
    },
    UnsignedIntegerOverflow {
        lhs: u64,
        rhs: u64,
        op: ArithOp,
    },
    ShiftOverflow {
        value: u64,
        shift: u32,
        width: u32,
    },
    OutOfBoundsIndex {
        index: i64,
        array_size: u64,
    },
    NullPointerDereference {
        is_write: bool,
    },
    AlignmentViolation {
        addr: u64,
        required: u64,
        actual: u64,
    },
    TypeMismatch {
        expected_type_hash: u64,
        actual_type_hash: u64,
    },
    UnreachableCode {
        ip: u64,
    },
    NegateOverflow {
        value: i64,
    },
    DivisionByZero {
        is_signed: bool,
    },
    LoadInvalidValue {
        value: u64,
        type_name_hash: u64,
    },
    InvalidBuiltin {
        kind: u32,
    },
    PointerOverflow {
        base: u64,
        result: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Negate,
}

#[derive(Debug, Clone, Copy)]
pub struct UbsanSourceLocation {
    pub file_hash: u64,
    pub line: u32,
    pub column: u32,
}

pub struct UbsanState {
    violations: Vec<(UbsanViolation, UbsanSourceLocation)>,
    enabled: AtomicBool,
    trap_on_violation: AtomicBool,
    violation_count: AtomicU64,
    suppressed_count: AtomicU64,
}

impl UbsanState {
    pub fn new() -> Self {
        Self {
            violations: Vec::new(),
            enabled: AtomicBool::new(false),
            trap_on_violation: AtomicBool::new(false),
            violation_count: AtomicU64::new(0),
            suppressed_count: AtomicU64::new(0),
        }
    }

    pub fn enable(&self, trap: bool) {
        self.enabled.store(true, Ordering::SeqCst);
        self.trap_on_violation.store(trap, Ordering::SeqCst);
    }

    pub fn report_violation(&mut self, violation: UbsanViolation, loc: UbsanSourceLocation) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        self.violation_count.fetch_add(1, Ordering::Relaxed);
        self.violations.push((violation, loc));
    }

    pub fn check_signed_add(
        &mut self,
        a: i64,
        b: i64,
        loc: UbsanSourceLocation,
    ) -> Result<i64, UbsanViolation> {
        match a.checked_add(b) {
            Some(result) => Ok(result),
            None => {
                let v = UbsanViolation::SignedIntegerOverflow {
                    lhs: a,
                    rhs: b,
                    op: ArithOp::Add,
                };
                self.report_violation(v, loc);
                Err(v)
            }
        }
    }

    pub fn check_signed_sub(
        &mut self,
        a: i64,
        b: i64,
        loc: UbsanSourceLocation,
    ) -> Result<i64, UbsanViolation> {
        match a.checked_sub(b) {
            Some(result) => Ok(result),
            None => {
                let v = UbsanViolation::SignedIntegerOverflow {
                    lhs: a,
                    rhs: b,
                    op: ArithOp::Sub,
                };
                self.report_violation(v, loc);
                Err(v)
            }
        }
    }

    pub fn check_signed_mul(
        &mut self,
        a: i64,
        b: i64,
        loc: UbsanSourceLocation,
    ) -> Result<i64, UbsanViolation> {
        match a.checked_mul(b) {
            Some(result) => Ok(result),
            None => {
                let v = UbsanViolation::SignedIntegerOverflow {
                    lhs: a,
                    rhs: b,
                    op: ArithOp::Mul,
                };
                self.report_violation(v, loc);
                Err(v)
            }
        }
    }

    pub fn check_unsigned_add(
        &mut self,
        a: u64,
        b: u64,
        loc: UbsanSourceLocation,
    ) -> Result<u64, UbsanViolation> {
        match a.checked_add(b) {
            Some(result) => Ok(result),
            None => {
                let v = UbsanViolation::UnsignedIntegerOverflow {
                    lhs: a,
                    rhs: b,
                    op: ArithOp::Add,
                };
                self.report_violation(v, loc);
                Err(v)
            }
        }
    }

    pub fn check_shift(
        &mut self,
        value: u64,
        shift: u32,
        width: u32,
        loc: UbsanSourceLocation,
    ) -> Result<u64, UbsanViolation> {
        if shift >= width {
            let v = UbsanViolation::ShiftOverflow {
                value,
                shift,
                width,
            };
            self.report_violation(v, loc);
            return Err(v);
        }
        Ok(value << shift)
    }

    pub fn check_array_bounds(
        &mut self,
        index: i64,
        size: u64,
        loc: UbsanSourceLocation,
    ) -> Result<(), UbsanViolation> {
        if index < 0 || index as u64 >= size {
            let v = UbsanViolation::OutOfBoundsIndex {
                index,
                array_size: size,
            };
            self.report_violation(v, loc);
            return Err(v);
        }
        Ok(())
    }

    pub fn check_null_deref(
        &mut self,
        ptr: u64,
        is_write: bool,
        loc: UbsanSourceLocation,
    ) -> Result<(), UbsanViolation> {
        if ptr == 0 {
            let v = UbsanViolation::NullPointerDereference { is_write };
            self.report_violation(v, loc);
            return Err(v);
        }
        Ok(())
    }

    pub fn check_alignment(
        &mut self,
        addr: u64,
        required: u64,
        loc: UbsanSourceLocation,
    ) -> Result<(), UbsanViolation> {
        let actual = addr & (required - 1);
        if actual != 0 {
            let v = UbsanViolation::AlignmentViolation {
                addr,
                required,
                actual,
            };
            self.report_violation(v, loc);
            return Err(v);
        }
        Ok(())
    }

    pub fn check_division(
        &mut self,
        divisor: i64,
        is_signed: bool,
        loc: UbsanSourceLocation,
    ) -> Result<(), UbsanViolation> {
        if divisor == 0 {
            let v = UbsanViolation::DivisionByZero { is_signed };
            self.report_violation(v, loc);
            return Err(v);
        }
        Ok(())
    }

    pub fn report_unreachable(&mut self, ip: u64, loc: UbsanSourceLocation) {
        let v = UbsanViolation::UnreachableCode { ip };
        self.report_violation(v, loc);
    }

    pub fn violation_count(&self) -> u64 {
        self.violation_count.load(Ordering::Relaxed)
    }
}

// ===========================================================================
// W^X Enforcement — No memory both Writable and Executable
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PagePermission {
    None,
    ReadOnly,
    ReadWrite,
    ReadExecute,
    Execute,
}

impl PagePermission {
    pub fn is_writable(self) -> bool {
        matches!(self, Self::ReadWrite)
    }

    pub fn is_executable(self) -> bool {
        matches!(self, Self::ReadExecute | Self::Execute)
    }

    pub fn violates_wx(self) -> bool {
        self.is_writable() && self.is_executable()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PageAuditEntry {
    pub address: u64,
    pub size: u64,
    pub permission: PagePermission,
    pub owner_pid: u32,
}

pub struct WxEnforcement {
    enabled: AtomicBool,
    enforce: AtomicBool,
    violations: AtomicU64,
    audit_log: Vec<PageAuditEntry>,
    page_table: BTreeMap<u64, PagePermission>,
}

impl WxEnforcement {
    pub fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            enforce: AtomicBool::new(false),
            violations: AtomicU64::new(0),
            audit_log: Vec::new(),
            page_table: BTreeMap::new(),
        }
    }

    pub fn enable(&self, enforce: bool) {
        self.enabled.store(true, Ordering::SeqCst);
        self.enforce.store(enforce, Ordering::SeqCst);
    }

    pub fn set_permission(&mut self, addr: u64, perm: PagePermission) -> Result<(), &'static str> {
        if self.enabled.load(Ordering::Relaxed) && perm.violates_wx() {
            self.violations.fetch_add(1, Ordering::Relaxed);
            if self.enforce.load(Ordering::Relaxed) {
                return Err("W^X violation: page cannot be both writable and executable");
            }
        }
        self.page_table.insert(addr, perm);
        Ok(())
    }

    pub fn get_permission(&self, addr: u64) -> Option<&PagePermission> {
        self.page_table.get(&addr)
    }

    pub fn audit_all_pages(&self) -> Vec<u64> {
        let mut violations = Vec::new();
        for (&addr, &perm) in &self.page_table {
            if perm.violates_wx() {
                violations.push(addr);
            }
        }
        violations
    }

    pub fn make_readonly(&mut self, addr: u64) {
        self.page_table.insert(addr, PagePermission::ReadOnly);
    }

    pub fn make_executable(&mut self, addr: u64) {
        self.page_table.insert(addr, PagePermission::ReadExecute);
    }

    pub fn violation_count(&self) -> u64 {
        self.violations.load(Ordering::Relaxed)
    }
}

// ===========================================================================
// Kernel Lockdown — restrict dangerous kernel interfaces
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockdownMode {
    None,
    Integrity,
    Confidentiality,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockdownRestriction {
    ModuleLoading,
    DevMem,
    KProbes,
    AcpiTables,
    PciBarAccess,
    IoPortAccess,
    MsrAccess,
    DebugFS,
    BpfWrite,
    PerfEventOpen,
    KexecLoad,
    HibernationImage,
    Ioperm,
    Iopl,
}

pub struct KernelLockdown {
    mode: LockdownMode,
    restrictions: [bool; 14],
    denial_count: AtomicU64,
    enabled: AtomicBool,
}

impl KernelLockdown {
    pub const fn new() -> Self {
        Self {
            mode: LockdownMode::None,
            restrictions: [false; 14],
            denial_count: AtomicU64::new(0),
            enabled: AtomicBool::new(false),
        }
    }

    pub fn set_mode(&mut self, mode: LockdownMode) {
        self.mode = mode;
        self.enabled
            .store(mode != LockdownMode::None, Ordering::SeqCst);

        for r in &mut self.restrictions {
            *r = false;
        }

        match mode {
            LockdownMode::None => {}
            LockdownMode::Integrity => {
                self.restrictions[LockdownRestriction::ModuleLoading as usize] = true;
                self.restrictions[LockdownRestriction::DevMem as usize] = true;
                self.restrictions[LockdownRestriction::KProbes as usize] = true;
                self.restrictions[LockdownRestriction::AcpiTables as usize] = true;
                self.restrictions[LockdownRestriction::KexecLoad as usize] = true;
                self.restrictions[LockdownRestriction::Ioperm as usize] = true;
                self.restrictions[LockdownRestriction::Iopl as usize] = true;
            }
            LockdownMode::Confidentiality => {
                for r in &mut self.restrictions {
                    *r = true;
                }
            }
        }
    }

    pub fn is_restricted(&self, restriction: LockdownRestriction) -> bool {
        if !self.enabled.load(Ordering::Relaxed) {
            return false;
        }
        let idx = restriction as usize;
        if idx < self.restrictions.len() {
            self.restrictions[idx]
        } else {
            false
        }
    }

    pub fn check_permission(&self, restriction: LockdownRestriction) -> Result<(), &'static str> {
        if self.is_restricted(restriction) {
            self.denial_count.fetch_add(1, Ordering::Relaxed);
            match restriction {
                LockdownRestriction::ModuleLoading => Err("lockdown: module loading denied"),
                LockdownRestriction::DevMem => Err("lockdown: /dev/mem access denied"),
                LockdownRestriction::KProbes => Err("lockdown: kprobes denied"),
                LockdownRestriction::AcpiTables => Err("lockdown: ACPI table modification denied"),
                LockdownRestriction::PciBarAccess => Err("lockdown: PCI BAR access denied"),
                LockdownRestriction::IoPortAccess => Err("lockdown: I/O port access denied"),
                LockdownRestriction::MsrAccess => Err("lockdown: MSR access denied"),
                LockdownRestriction::DebugFS => Err("lockdown: debugfs access denied"),
                LockdownRestriction::BpfWrite => Err("lockdown: BPF write denied"),
                LockdownRestriction::PerfEventOpen => Err("lockdown: perf_event_open denied"),
                LockdownRestriction::KexecLoad => Err("lockdown: kexec_load denied"),
                LockdownRestriction::HibernationImage => Err("lockdown: hibernation image denied"),
                LockdownRestriction::Ioperm => Err("lockdown: ioperm denied"),
                LockdownRestriction::Iopl => Err("lockdown: iopl denied"),
            }
        } else {
            Ok(())
        }
    }

    pub fn mode(&self) -> LockdownMode {
        self.mode
    }

    pub fn denial_count(&self) -> u64 {
        self.denial_count.load(Ordering::Relaxed)
    }
}

// ===========================================================================
// SMAP/SMEP — Supervisor Mode Access/Execution Prevention
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmapSmepViolation {
    SupervisorRead { address: u64 },
    SupervisorWrite { address: u64 },
    SupervisorExecute { address: u64 },
}

pub struct SmapSmepState {
    smap_enabled: AtomicBool,
    smep_enabled: AtomicBool,
    smap_violations: AtomicU64,
    smep_violations: AtomicU64,
    user_space_start: u64,
    user_space_end: u64,
    ac_flag_set: AtomicBool,
}

impl SmapSmepState {
    pub const fn new() -> Self {
        Self {
            smap_enabled: AtomicBool::new(false),
            smep_enabled: AtomicBool::new(false),
            smap_violations: AtomicU64::new(0),
            smep_violations: AtomicU64::new(0),
            user_space_start: 0x0000_0000_0000_0000,
            user_space_end: 0x0000_7FFF_FFFF_FFFF,
            ac_flag_set: AtomicBool::new(false),
        }
    }

    pub fn enable_smap(&self) {
        // REAL hardware SMAP: set CR4.SMAP (bit 21) on this CPU if supported so
        // any ring-0 read/write of a user page outside the stac/clac-bracketed
        // uaccess chokepoint faults. `cpu_features::enable_smap` arms the
        // stac/clac copy stubs BEFORE flipping CR4 (ordering contract there).
        // The flag reflects the actual CR4 bit (read back), not an intent.
        let active = crate::cpu_features::enable_smap();
        self.smap_enabled.store(active, Ordering::SeqCst);
    }

    pub fn enable_smep(&self) {
        // REAL hardware SMEP: set CR4.SMEP (bit 20) on this CPU if supported so a
        // ring-0 execute of a user page faults. The flag reflects the actual CR4
        // bit (read back), not an intent — no more software simulation.
        let active = crate::cpu_features::enable_smep();
        self.smep_enabled.store(active, Ordering::SeqCst);
    }

    pub fn is_user_address(&self, addr: u64) -> bool {
        addr >= self.user_space_start && addr <= self.user_space_end
    }

    pub fn stac(&self) {
        self.ac_flag_set.store(true, Ordering::SeqCst);
    }

    pub fn clac(&self) {
        self.ac_flag_set.store(false, Ordering::SeqCst);
    }

    pub fn check_supervisor_access(
        &self,
        addr: u64,
        is_write: bool,
    ) -> Result<(), SmapSmepViolation> {
        if !self.smap_enabled.load(Ordering::Relaxed) {
            return Ok(());
        }
        if self.ac_flag_set.load(Ordering::Relaxed) {
            return Ok(());
        }
        if self.is_user_address(addr) {
            self.smap_violations.fetch_add(1, Ordering::Relaxed);
            if is_write {
                return Err(SmapSmepViolation::SupervisorWrite { address: addr });
            } else {
                return Err(SmapSmepViolation::SupervisorRead { address: addr });
            }
        }
        Ok(())
    }

    pub fn check_supervisor_execute(&self, addr: u64) -> Result<(), SmapSmepViolation> {
        if !self.smep_enabled.load(Ordering::Relaxed) {
            return Ok(());
        }
        if self.is_user_address(addr) {
            self.smep_violations.fetch_add(1, Ordering::Relaxed);
            return Err(SmapSmepViolation::SupervisorExecute { address: addr });
        }
        Ok(())
    }

    pub fn smap_violation_count(&self) -> u64 {
        self.smap_violations.load(Ordering::Relaxed)
    }

    pub fn smep_violation_count(&self) -> u64 {
        self.smep_violations.load(Ordering::Relaxed)
    }
}

// ===========================================================================
// Spectre/Meltdown Mitigations
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MitigationStatus {
    NotRequired,
    Disabled,
    Enabled,
    FirmwareSupport,
    NotSupported,
}

#[derive(Debug, Clone, Copy)]
pub struct SpectreState {
    pub retpoline: MitigationStatus,
    pub ibrs: MitigationStatus,
    pub ibpb: MitigationStatus,
    pub stibp: MitigationStatus,
    pub ssbd: MitigationStatus,
    pub l1tf_flush: MitigationStatus,
    pub mds_clear: MitigationStatus,
    pub taa_mitigation: MitigationStatus,
    pub srbds_mitigation: MitigationStatus,
    pub speculative_store_bypass: MitigationStatus,
}

impl SpectreState {
    pub const fn new() -> Self {
        Self {
            retpoline: MitigationStatus::Disabled,
            ibrs: MitigationStatus::Disabled,
            ibpb: MitigationStatus::Disabled,
            stibp: MitigationStatus::Disabled,
            ssbd: MitigationStatus::Disabled,
            l1tf_flush: MitigationStatus::Disabled,
            mds_clear: MitigationStatus::Disabled,
            taa_mitigation: MitigationStatus::Disabled,
            srbds_mitigation: MitigationStatus::Disabled,
            speculative_store_bypass: MitigationStatus::Disabled,
        }
    }

    // The former per-flag setters (enable_retpoline/enable_ibrs/…) were removed:
    // they set MitigationStatus::Enabled with NO MSR behind them (false
    // advertising). Real branch-speculation state is now programmed and
    // read-back-verified in `SpectreManager::enable_all_mitigations`.

    pub fn flush_l1d_on_vmentry(&self) -> bool {
        self.l1tf_flush == MitigationStatus::Enabled
    }

    pub fn clear_buffers_on_context_switch(&self) -> bool {
        self.mds_clear == MitigationStatus::Enabled
    }

    pub fn issue_ibpb_on_task_switch(&self) -> bool {
        self.ibpb == MitigationStatus::Enabled
    }

    /// "Fully mitigated" = every branch-speculation defense this CPU SUPPORTS
    /// is active. A `NotSupported`/`NotRequired`/`FirmwareSupport` mitigation
    /// does not count against us (the hardware either cannot do it or is not
    /// vulnerable — e.g. the MDS/L1TF family is Intel-only, `NotRequired` on
    /// AMD Zen); only a `Disabled` mitigation that could be on drags this to
    /// false. This is the HONEST definition — it reflects the read-back MSR
    /// state set by `enable_all_mitigations`, not an optimistic flag.
    pub fn is_fully_mitigated(&self) -> bool {
        let ok = |m: MitigationStatus| {
            matches!(
                m,
                MitigationStatus::Enabled
                    | MitigationStatus::NotSupported
                    | MitigationStatus::NotRequired
                    | MitigationStatus::FirmwareSupport
            )
        };
        ok(self.ibrs)
            && ok(self.stibp)
            && ok(self.ssbd)
            && ok(self.l1tf_flush)
            && ok(self.mds_clear)
    }
}

pub struct SpectreManager {
    pub state: SpectreState,
    pub context_switch_count: AtomicU64,
    pub ibpb_issued_count: AtomicU64,
    pub l1d_flush_count: AtomicU64,
    pub mds_clear_count: AtomicU64,
}

impl SpectreManager {
    pub const fn new() -> Self {
        Self {
            state: SpectreState::new(),
            context_switch_count: AtomicU64::new(0),
            ibpb_issued_count: AtomicU64::new(0),
            l1d_flush_count: AtomicU64::new(0),
            mds_clear_count: AtomicU64::new(0),
        }
    }

    pub fn on_context_switch(&self) {
        self.context_switch_count.fetch_add(1, Ordering::Relaxed);

        if self.state.issue_ibpb_on_task_switch() {
            self.issue_ibpb();
        }
        if self.state.clear_buffers_on_context_switch() {
            self.clear_cpu_buffers();
        }
    }

    pub fn on_vm_entry(&self) {
        if self.state.flush_l1d_on_vmentry() {
            self.flush_l1d();
        }
    }

    fn issue_ibpb(&self) {
        // Real barrier: flush the indirect-branch predictors via IA32_PRED_CMD
        // (no-op + false when the CPU lacks IBPB). The counter records issuance
        // for /proc telemetry. Only reached when `issue_ibpb_on_task_switch()`
        // is true, which requires ibpb == Enabled (auto-issuance is a deferred
        // scheduler hook — see enable_all_mitigations).
        crate::cpu_features::issue_ibpb();
        self.ibpb_issued_count.fetch_add(1, Ordering::Relaxed);
    }

    fn flush_l1d(&self) {
        self.l1d_flush_count.fetch_add(1, Ordering::Relaxed);
    }

    fn clear_cpu_buffers(&self) {
        self.mds_clear_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Program the REAL branch-speculation MSRs on the current (BSP) CPU and
    /// record the HARDWARE read-back — not an optimistic flag. APs program the
    /// same MSRs from `smp::ap_entry` (SPEC_CTRL is per-CPU); this method owns
    /// the status bookkeeping. Replaces the former software-simulated enables
    /// (which set every field to `Enabled` with no MSR behind them and made
    /// `is_fully_mitigated()` return true dishonestly).
    pub fn enable_all_mitigations(&mut self) {
        // 1. Persistent branch-speculation defenses: write IBRS/STIBP/SSBD for
        //    whatever this CPU advertises, then classify from the read-back.
        let readback = crate::cpu_features::enable_spec_ctrl();
        let sup = crate::cpu_features::spec_ctrl_support();
        let classify = |supported: bool, active: bool| {
            if !supported {
                MitigationStatus::NotSupported
            } else if active {
                MitigationStatus::Enabled
            } else {
                MitigationStatus::Disabled
            }
        };
        self.state.ibrs = classify(sup.ibrs, readback & (1 << 0) != 0);
        self.state.stibp = classify(sup.stibp, readback & (1 << 1) != 0);
        self.state.ssbd = classify(sup.ssbd, readback & (1 << 2) != 0);
        self.state.speculative_store_bypass = self.state.ssbd;

        // 2. IBPB is a one-shot barrier, not a persistent MSR bit. The issue
        //    path (cpu_features::issue_ibpb) is real, but auto-issuance on every
        //    context switch carries a real hot-path cost, so per-switch firing
        //    is deferred to a scheduler hook (MasterChecklist Phase 4: IBPB on
        //    cross-domain switch). Report the capability honestly rather than
        //    claiming it is actively firing.
        self.state.ibpb = if sup.ibpb {
            MitigationStatus::FirmwareSupport
        } else {
            MitigationStatus::NotSupported
        };

        // 3. Retpoline is a BUILD-TIME codegen mitigation: it requires rebuilding
        //    core/alloc with `-Zretpoline` (build-std), which this kernel does
        //    NOT do. There is no runtime toggle that can conjure the thunks, so
        //    report it honestly as `NotSupported` instead of the former
        //    always-`Enabled` lie. (A status flag flipped without the codegen
        //    behind it would just be a new lie — the persistent IBRS bit above
        //    is our real indirect-branch defense.)
        self.state.retpoline = MitigationStatus::NotSupported;

        // 4. The microarchitectural-data-sampling family (L1TF, MDS, TAA,
        //    SRBDS) is Intel-specific silicon errata. AMD parts (the Athena
        //    Zen4 target) are architecturally NOT vulnerable → `NotRequired`.
        //    On Intel we have not yet wired the real VERW / L1D_FLUSH sequences,
        //    so we report `Disabled` (honest "not implemented") rather than a
        //    fake counter-only `Enabled`.
        let mds_family = if crate::msr::is_intel() {
            MitigationStatus::Disabled
        } else {
            MitigationStatus::NotRequired
        };
        self.state.l1tf_flush = mds_family;
        self.state.mds_clear = mds_family;
        self.state.taa_mitigation = mds_family;
        self.state.srbds_mitigation = mds_family;
    }
}

// ===========================================================================
// Global HARDENING Manager
// ===========================================================================

pub struct HardeningManager {
    pub kaslr: KaslrState,
    pub canary: &'static StackCanaryManager,
    pub cfi: CfiManager,
    pub kasan: KasanState,
    pub kfence: KfenceState,
    pub ubsan: UbsanState,
    pub wx: WxEnforcement,
    pub lockdown: KernelLockdown,
    pub smap_smep: &'static SmapSmepState,
    pub spectre: SpectreManager,
    pub initialized: bool,
}

static CANARY_MANAGER: StackCanaryManager = StackCanaryManager::new();
static SMAP_SMEP: SmapSmepState = SmapSmepState::new();

pub static HARDENING: Mutex<Option<HardeningManager>> = Mutex::new(None);

impl HardeningManager {
    pub fn new() -> Self {
        Self {
            kaslr: KaslrState::new(),
            canary: &CANARY_MANAGER,
            cfi: CfiManager::new(),
            kasan: KasanState::new(),
            kfence: KfenceState::new(KFENCE_POOL_BASE),
            ubsan: UbsanState::new(),
            wx: WxEnforcement::new(),
            lockdown: KernelLockdown::new(),
            smap_smep: &SMAP_SMEP,
            spectre: SpectreManager::new(),
            initialized: false,
        }
    }

    pub fn init_all(&mut self) {
        self.kaslr.detect_entropy_source();
        let entropy = self.kaslr.get_raw_entropy();
        self.kaslr.randomize(entropy);

        self.canary.init(entropy ^ 0xDEAD_BEEF_1337_CAFE);

        self.cfi.enable(true);

        // HONESTY: KASAN and KFENCE are software detectors whose runtime data
        // structures exist in this file, but the compiler-inserted access checks
        // (shadow-memory loads for KASAN, sampled guard-page allocation for
        // KFENCE) are NOT wired into the allocator / codegen pipeline. We only
        // arm them when the corresponding nightly build feature is present so we
        // never advertise a sanitizer that is not actually instrumenting code.
        #[cfg(feature = "kasan")]
        {
            self.kasan.enable();
        }
        #[cfg(feature = "kfence")]
        {
            self.kfence.enable();
        }

        // UBSAN: Rust's own overflow checks (cfg(debug_assertions)) are our only
        // real undefined-behavior guard. The UbsanState helpers are opt-in call
        // sites, so we arm the bookkeeping only in debug builds where overflow
        // checks are also active. We never trap so a stray report cannot panic.
        if cfg!(debug_assertions) {
            self.ubsan.enable(false);
        }

        self.wx.enable(true);

        self.lockdown.set_mode(LockdownMode::Integrity);

        SMAP_SMEP.enable_smap();
        SMAP_SMEP.enable_smep();
        // UMIP (CR4.UMIP): block userspace SGDT/SIDT/SLDT/STR/SMSW so the
        // GDT/IDT/LDT/TSS linear addresses can't be leaked to defeat KASLR.
        // Same hardware-trap vein as SMEP/SMAP; per-CPU (APs enable their own
        // in ap_entry). No status stored here — the read-back smoketest is the
        // source of truth.
        crate::cpu_features::enable_umip();

        self.spectre.enable_all_mitigations();

        self.initialized = true;
    }

    /// Is `restriction` enforced by the current lockdown mode? (Side-effect-free;
    /// does not increment the denial counter.) Phase 4.10 enforcement query.
    pub fn lockdown_restricts(&self, restriction: LockdownRestriction) -> bool {
        self.lockdown.is_restricted(restriction)
    }

    /// Current kernel lockdown mode.
    pub fn lockdown_mode(&self) -> LockdownMode {
        self.lockdown.mode()
    }

    /// True if KASAN shadow-memory instrumentation is compiled in AND the heap
    /// shadow region is actually mapped + the allocator is instrumenting (alloc
    /// unpoisons / dealloc poisons through the real shadow writers). Default
    /// builds return `false` (the feature is off). With the feature on, this
    /// reports the LIVE shadow state — mirroring `kfence_instrumented()` — so the
    /// honesty audit only passes once real instrumentation is in place, never on
    /// armed-but-unwired bookkeeping.
    pub fn kasan_instrumented() -> bool {
        #[cfg(feature = "kasan")]
        {
            crate::memory::allocator::kasan_is_live()
        }
        #[cfg(not(feature = "kasan"))]
        {
            false
        }
    }

    /// True if KFENCE sampled guard-page detection is compiled in AND the
    /// guard-page pool is actually mapped + armed. Default builds return
    /// `false` (the feature is off). With the feature on, this reports the LIVE
    /// pool state — so the honesty audit only passes once real instrumentation
    /// (mapped guard pages diverting sampled allocations) is in place, never on
    /// armed-but-unmapped bookkeeping.
    pub fn kfence_instrumented() -> bool {
        #[cfg(feature = "kfence")]
        {
            sampler::is_live()
        }
        #[cfg(not(feature = "kfence"))]
        {
            false
        }
    }

    /// True if UBSAN-equivalent overflow checks are active. Rust inserts these
    /// in debug builds via `cfg(debug_assertions)`; release builds drop them.
    pub fn ubsan_overflow_checks() -> bool {
        cfg!(debug_assertions)
    }

    /// True if the kernel was built with compiler KCFI. KCFI requires the
    /// nightly `-Z sanitizer=kcfi` flag; the matching `kcfi` build feature must
    /// be enabled alongside it so this reflects reality. The unstable
    /// `cfg(sanitize)` predicate cannot be queried on this stable toolchain, so
    /// we gate on the explicit feature instead.
    /// The software `CfiManager` whitelist in this file is a separate, coarse
    /// mechanism and is NOT the same as compiler KCFI.
    pub fn kcfi_instrumented() -> bool {
        cfg!(feature = "kcfi")
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    pub fn security_status(&self) -> HardeningStatus {
        HardeningStatus {
            kaslr_enabled: self.kaslr.enabled,
            canary_enabled: self.canary.enabled.load(Ordering::Relaxed),
            cfi_enabled: self.cfi.enabled.load(Ordering::Relaxed),
            kasan_enabled: self.kasan.enabled.load(Ordering::Relaxed),
            kasan_instrumented: Self::kasan_instrumented(),
            kfence_enabled: self.kfence.enabled.load(Ordering::Relaxed),
            kfence_instrumented: Self::kfence_instrumented(),
            ubsan_enabled: self.ubsan.enabled.load(Ordering::Relaxed),
            ubsan_overflow_checks: Self::ubsan_overflow_checks(),
            kcfi_instrumented: Self::kcfi_instrumented(),
            wx_enforced: self.wx.enforce.load(Ordering::Relaxed),
            lockdown_mode: self.lockdown.mode(),
            smap_enabled: SMAP_SMEP.smap_enabled.load(Ordering::Relaxed),
            smep_enabled: SMAP_SMEP.smep_enabled.load(Ordering::Relaxed),
            spectre_mitigated: self.spectre.state.is_fully_mitigated(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HardeningStatus {
    pub kaslr_enabled: bool,
    pub canary_enabled: bool,
    pub cfi_enabled: bool,
    /// KASAN bookkeeping armed (only when built with feature "kasan").
    pub kasan_enabled: bool,
    /// KASAN compiler shadow-memory instrumentation actually present.
    pub kasan_instrumented: bool,
    /// KFENCE bookkeeping armed (only when built with feature "kfence").
    pub kfence_enabled: bool,
    /// KFENCE sampled guard-page detector actually present.
    pub kfence_instrumented: bool,
    /// UbsanState bookkeeping armed (debug builds only).
    pub ubsan_enabled: bool,
    /// Rust overflow checks active (`cfg(debug_assertions)`).
    pub ubsan_overflow_checks: bool,
    /// Compiler KCFI (`-Z sanitizer=kcfi`) present.
    pub kcfi_instrumented: bool,
    pub wx_enforced: bool,
    pub lockdown_mode: LockdownMode,
    pub smap_enabled: bool,
    pub smep_enabled: bool,
    pub spectre_mitigated: bool,
}

pub fn init() {
    let mut mgr = HardeningManager::new();
    mgr.init_all();
    *HARDENING.lock() = Some(mgr);

    // Map + arm the real KFENCE guard-page pool (feature = "kfence" only). With
    // the feature off this call does not exist and no pool is mapped, so
    // `kfence_instrumented()` (= pool live) honestly reports false.
    #[cfg(feature = "kfence")]
    {
        sampler::init();
    }

    // KASAN shadow is mapped + armed inside `allocator::init_heap` (which runs
    // before this). With the feature off this block does not exist and no shadow
    // is instrumented, so `kasan_instrumented()` (= shadow live) honestly reports
    // false. We only announce liveness here.
    #[cfg(feature = "kasan")]
    {
        if kasan::is_live() {
            crate::serial_println!(
                "[kasan] shadow armed: start={:#x} size={} bytes (1 shadow byte / 8 heap bytes, alloc unpoison + dealloc poison + quarantine)",
                crate::memory::allocator::SHADOW_START,
                crate::memory::allocator::SHADOW_SIZE
            );
        } else {
            crate::serial_println!("[kasan] shadow NOT live (heap shadow unmapped)");
        }
    }
}

/// Snapshot of hardening state for `/proc/raeen/hardening`.
/// Required by `kernelchecklist.md` R3 (every subsystem gets a procfs endpoint).
pub fn status() -> Option<HardeningStatus> {
    HARDENING.lock().as_ref().map(|m| m.security_status())
}

/// Returns `true` if no hardening feature is falsely advertised — i.e. nothing
/// is reported as "enabled" unless its real instrumentation is also present.
///
/// The audit fails if any of the following lie:
///   * KASAN bookkeeping armed without compiler shadow-memory instrumentation.
///   * KFENCE bookkeeping armed without the sampled guard-page detector.
///   * UBSAN bookkeeping armed without Rust overflow checks (`debug_assertions`).
/// CFI/CFI-whitelist is software-only and reported as such, never as KCFI.
pub fn honesty_audit() -> bool {
    let s = match status() {
        Some(s) => s,
        None => return true, // nothing initialized => nothing falsely claimed
    };
    // A detector that is "enabled" but not actually instrumenting code is a lie.
    if s.kasan_enabled && !s.kasan_instrumented {
        return false;
    }
    if s.kfence_enabled && !s.kfence_instrumented {
        return false;
    }
    if s.ubsan_enabled && !s.ubsan_overflow_checks {
        return false;
    }
    true
}

/// Boot smoke test: verifies the hardening manager initialized and that the
/// honesty audit passes (no falsely advertised sanitizers).
/// R10: invoked from `kernel_main`.
pub fn run_boot_smoketest() {
    // Emit the precise honesty line so security audits can grep for it:
    //   [hardening] honesty_audit: kasan=false kfence=false ubsan=BOOL kcfi=false -> PASS
    let kasan = HardeningManager::kasan_instrumented();
    let kfence = HardeningManager::kfence_instrumented();
    let ubsan = HardeningManager::ubsan_overflow_checks();
    let kcfi = HardeningManager::kcfi_instrumented();
    let honest = honesty_audit();
    crate::serial_println!(
        "[hardening] honesty_audit: kasan={} kfence={} ubsan={} kcfi={} -> {}",
        kasan,
        kfence,
        ubsan,
        kcfi,
        if honest { "PASS" } else { "FAIL" }
    );
    match status() {
        Some(s) => crate::serial_println!(
            "[hardening] run_boot_smoketest: kaslr={} canary={} cfi={} wx={} smap={} smep={} spectre={} -> {}",
            s.kaslr_enabled,
            s.canary_enabled,
            s.cfi_enabled,
            s.wx_enforced,
            s.smap_enabled,
            s.smep_enabled,
            s.spectre_mitigated,
            if honest { "PASS" } else { "FAIL" }
        ),
        None => crate::serial_println!("[hardening] run_boot_smoketest: NOT INITIALIZED -> FAIL"),
    }

    // Phase 4.10: prove lockdown actually ENFORCES the dangerous-interface
    // restrictions. AthenaOS exposes no syscall for module loading, /dev/mem, or
    // kexec (structurally absent), and the lockdown gate marks them restricted on
    // top — so any future path that checks `check_permission` is denied.
    {
        let g = HARDENING.lock();
        if let Some(m) = g.as_ref() {
            let module_load = m.lockdown_restricts(LockdownRestriction::ModuleLoading);
            let dev_mem = m.lockdown_restricts(LockdownRestriction::DevMem);
            let kexec = m.lockdown_restricts(LockdownRestriction::KexecLoad);
            crate::serial_println!(
                "[hardening] lockdown enforce: mode={:?} module_load_denied={} devmem_denied={} kexec_denied={} (no syscall exposes these) -> {}",
                m.lockdown_mode(),
                module_load,
                dev_mem,
                kexec,
                if module_load && dev_mem && kexec { "PASS" } else { "FAIL" }
            );
        }
    }
}

/// Text dump for `/proc/raeen/hardening`.
pub fn dump_text() -> alloc::string::String {
    use alloc::string::String;
    let s = match status() {
        Some(s) => s,
        None => return String::from("# hardening manager not initialized\n"),
    };
    let mut out = String::new();
    out.push_str("# AthenaOS kernel hardening (Concept §Security)\n");
    let row = |out: &mut String, name: &str, on: bool| {
        out.push_str(&alloc::format!(
            "{:<24} {}\n",
            name,
            if on { "enabled" } else { "off" }
        ));
    };
    row(&mut out, "KASLR", s.kaslr_enabled);
    row(&mut out, "stack_canary", s.canary_enabled);
    row(&mut out, "CFI (software)", s.cfi_enabled);
    row(&mut out, "KASAN", s.kasan_enabled);
    row(&mut out, "KFENCE", s.kfence_enabled);
    row(&mut out, "UBSAN", s.ubsan_enabled);
    row(&mut out, "W^X", s.wx_enforced);
    row(&mut out, "SMAP", s.smap_enabled);
    row(&mut out, "SMEP", s.smep_enabled);
    row(&mut out, "Spectre mitig.", s.spectre_mitigated);
    out.push_str(&alloc::format!(
        "lockdown_mode            {:?}\n",
        s.lockdown_mode
    ));

    // -----------------------------------------------------------------------
    // HONESTY NOTICE — what is ACTUALLY on vs. what was historically advertised
    // -----------------------------------------------------------------------
    out.push_str("\n# === HONESTY NOTICE ===\n");
    out.push_str("# This section states what is REALLY instrumenting code, not\n");
    out.push_str("# merely which in-memory bookkeeping structs were constructed.\n");
    let claim = |out: &mut String, name: &str, real: bool, detail: &str| {
        out.push_str(&alloc::format!(
            "{:<26} {:<6} ({})\n",
            name,
            if real { "true" } else { "false" },
            detail
        ));
    };
    claim(
        &mut out,
        "kasan.shadow_memory",
        s.kasan_instrumented,
        if s.kasan_instrumented {
            "shadow mapping + alloc instrumentation active"
        } else {
            "instrumentation not yet wired"
        },
    );
    claim(
        &mut out,
        "kfence.detector",
        s.kfence_instrumented,
        if s.kfence_instrumented {
            "sampled guard-page allocator active"
        } else {
            "detector not yet implemented"
        },
    );
    claim(
        &mut out,
        "ubsan.overflow_checks",
        s.ubsan_overflow_checks,
        "debug_overflow_checks=cfg(debug_assertions)",
    );
    claim(
        &mut out,
        "kcfi.compiler_sanitizer",
        s.kcfi_instrumented,
        if s.kcfi_instrumented {
            "-Z sanitizer=kcfi present"
        } else {
            "requires nightly -Z sanitizer=kcfi; software CfiManager only"
        },
    );
    out.push_str(&alloc::format!(
        "honesty_audit              {}\n",
        if honesty_audit() {
            "PASS (no false advertising)"
        } else {
            "FAIL (false claim detected)"
        }
    ));
    out
}
