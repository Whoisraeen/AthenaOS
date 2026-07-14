//! Kexec and crash dump subsystem for AthenaOS.
//!
//! Fast kernel reboot via kexec_load / kexec_execute, crash kernel reservation
//! with `crashkernel=` parsing, kdump with /proc/vmcore ELF core format,
//! VMCOREINFO, purgatory SHA-256 integrity checks, makedumpfile support,
//! panic notifier chain, and CMA-based memory reservation.

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

// ───────────────────────────────────────────────────────────────────────────────
// 1. Kernel Image Formats
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelFormat {
    Elf64,
    BzImage,
    RawBinary,
    Unknown,
}

impl KernelFormat {
    pub fn detect(data: &[u8]) -> Self {
        if data.len() < 4 {
            return Self::Unknown;
        }
        if data[0..4] == [0x7f, b'E', b'L', b'F'] {
            return Self::Elf64;
        }
        // bzImage magic at offset 0x202: "HdrS"
        if data.len() > 0x206 && data[0x202..0x206] == [0x48, 0x64, 0x72, 0x53] {
            return Self::BzImage;
        }
        Self::RawBinary
    }
}

#[derive(Debug, Clone)]
pub struct ElfHeader64 {
    pub entry_point: u64,
    pub phdr_offset: u64,
    pub phdr_count: u16,
    pub shdr_offset: u64,
    pub shdr_count: u16,
    pub machine: u16,
}

impl ElfHeader64 {
    pub fn parse(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < 64 {
            return Err("ELF header too short");
        }
        if data[0..4] != [0x7f, b'E', b'L', b'F'] {
            return Err("not an ELF file");
        }
        if data[4] != 2 {
            return Err("not ELF64");
        }

        let entry = u64::from_le_bytes([
            data[24], data[25], data[26], data[27], data[28], data[29], data[30], data[31],
        ]);
        let phoff = u64::from_le_bytes([
            data[32], data[33], data[34], data[35], data[36], data[37], data[38], data[39],
        ]);
        let shoff = u64::from_le_bytes([
            data[40], data[41], data[42], data[43], data[44], data[45], data[46], data[47],
        ]);
        let machine = u16::from_le_bytes([data[18], data[19]]);
        let phnum = u16::from_le_bytes([data[56], data[57]]);
        let shnum = u16::from_le_bytes([data[60], data[61]]);

        Ok(Self {
            entry_point: entry,
            phdr_offset: phoff,
            phdr_count: phnum,
            shdr_offset: shoff,
            shdr_count: shnum,
            machine,
        })
    }
}

#[derive(Debug, Clone)]
pub struct BzImageHeader {
    pub setup_sects: u8,
    pub kernel_size: u32,
    pub loadflags: u8,
    pub cmd_line_ptr: u32,
    pub initrd_addr: u32,
    pub initrd_size: u32,
    pub relocatable: bool,
}

impl BzImageHeader {
    pub fn parse(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < 0x260 {
            return Err("bzImage header too short");
        }
        if data[0x202..0x206] != [0x48, 0x64, 0x72, 0x53] {
            return Err("missing HdrS magic");
        }
        let setup_sects = data[0x1f1];
        let loadflags = data[0x211];
        let kernel_size = u32::from_le_bytes([
            data[0x260 - 4],
            data[0x260 - 3],
            data[0x260 - 2],
            data[0x260 - 1],
        ]);
        Ok(Self {
            setup_sects,
            kernel_size,
            loadflags,
            cmd_line_ptr: 0,
            initrd_addr: 0,
            initrd_size: 0,
            relocatable: (loadflags & 0x01) != 0,
        })
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 2. Kexec Segments
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentType {
    Text,
    Data,
    Initrd,
    CommandLine,
    SetupData,
    Purgatory,
}

#[derive(Debug, Clone)]
pub struct KexecSegment {
    pub seg_type: SegmentType,
    pub buf: Vec<u8>,
    pub mem_addr: u64,
    pub mem_size: u64,
    pub hash: [u8; 32],
}

impl KexecSegment {
    pub fn new(seg_type: SegmentType, data: &[u8], mem_addr: u64) -> Self {
        let hash = Self::sha256(data);
        let mem_size = (data.len() as u64 + 0xFFF) & !0xFFF;
        Self {
            seg_type,
            buf: Vec::from(data),
            mem_addr,
            mem_size,
            hash,
        }
    }

    fn sha256(data: &[u8]) -> [u8; 32] {
        let mut h: [u32; 8] = [
            0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
            0x5be0cd19,
        ];
        let total_bits = (data.len() as u64) * 8;
        let mut padded = Vec::from(data);
        padded.push(0x80);
        while (padded.len() % 64) != 56 {
            padded.push(0);
        }
        padded.extend_from_slice(&total_bits.to_be_bytes());

        let k: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];

        for chunk in padded.chunks(64) {
            let mut w = [0u32; 64];
            for i in 0..16 {
                w[i] = u32::from_be_bytes([
                    chunk[i * 4],
                    chunk[i * 4 + 1],
                    chunk[i * 4 + 2],
                    chunk[i * 4 + 3],
                ]);
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                w[i] = w[i - 16]
                    .wrapping_add(s0)
                    .wrapping_add(w[i - 7])
                    .wrapping_add(s1);
            }
            let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
                (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
            for i in 0..64 {
                let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
                let ch = (e & f) ^ ((!e) & g);
                let temp1 = hh
                    .wrapping_add(s1)
                    .wrapping_add(ch)
                    .wrapping_add(k[i])
                    .wrapping_add(w[i]);
                let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
                let maj = (a & b) ^ (a & c) ^ (b & c);
                let temp2 = s0.wrapping_add(maj);
                hh = g;
                g = f;
                f = e;
                e = d.wrapping_add(temp1);
                d = c;
                c = b;
                b = a;
                a = temp1.wrapping_add(temp2);
            }
            h[0] = h[0].wrapping_add(a);
            h[1] = h[1].wrapping_add(b);
            h[2] = h[2].wrapping_add(c);
            h[3] = h[3].wrapping_add(d);
            h[4] = h[4].wrapping_add(e);
            h[5] = h[5].wrapping_add(f);
            h[6] = h[6].wrapping_add(g);
            h[7] = h[7].wrapping_add(hh);
        }

        let mut result = [0u8; 32];
        for (i, word) in h.iter().enumerate() {
            result[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        result
    }

    pub fn verify_integrity(&self) -> bool {
        // Constant-time hash comparison (defense-in-depth for the kexec image
        // authenticity gate — never leak the expected digest via compare timing).
        let computed = Self::sha256(&self.buf);
        crate::crypto::ct_eq(&computed, &self.hash)
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 3. Purgatory
// ───────────────────────────────────────────────────────────────────────────────

pub struct Purgatory {
    pub code: Vec<u8>,
    pub segments: Vec<(u64, [u8; 32])>,
    pub entry: u64,
}

impl Purgatory {
    pub fn new() -> Self {
        Self {
            code: Vec::new(),
            segments: Vec::new(),
            entry: 0,
        }
    }

    pub fn add_segment_check(&mut self, addr: u64, hash: [u8; 32]) {
        self.segments.push((addr, hash));
    }

    pub fn verify_all(&self, memory: &dyn Fn(u64, usize) -> Vec<u8>) -> bool {
        for &(addr, ref expected_hash) in &self.segments {
            let data = memory(addr, 4096);
            let actual = KexecSegment::sha256(&data);
            if actual != *expected_hash {
                return false;
            }
        }
        true
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 4. Crash Kernel Reservation
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct CrashKernelRegion {
    pub start: u64,
    pub size: u64,
    pub in_use: bool,
}

pub fn parse_crashkernel_param(param: &str) -> Option<(u64, Option<u64>)> {
    let param = param.trim();
    if let Some(at_pos) = param.find('@') {
        let size_str = &param[..at_pos];
        let offset_str = &param[at_pos + 1..];
        let size = parse_size(size_str)?;
        let offset = parse_size(offset_str)?;
        Some((size, Some(offset)))
    } else {
        let size = parse_size(param)?;
        Some((size, None))
    }
}

fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (num_str, multiplier) = if s.ends_with('G') || s.ends_with('g') {
        (&s[..s.len() - 1], 1024 * 1024 * 1024u64)
    } else if s.ends_with('M') || s.ends_with('m') {
        (&s[..s.len() - 1], 1024 * 1024u64)
    } else if s.ends_with('K') || s.ends_with('k') {
        (&s[..s.len() - 1], 1024u64)
    } else {
        (s, 1u64)
    };
    let mut val = 0u64;
    for b in num_str.bytes() {
        if b < b'0' || b > b'9' {
            return None;
        }
        val = val.checked_mul(10)?.checked_add((b - b'0') as u64)?;
    }
    val.checked_mul(multiplier)
}

// ───────────────────────────────────────────────────────────────────────────────
// 5. VMCOREINFO
// ───────────────────────────────────────────────────────────────────────────────

pub struct VmcoreInfo {
    pub entries: BTreeMap<String, String>,
}

impl VmcoreInfo {
    pub fn new() -> Self {
        let mut entries = BTreeMap::new();
        entries.insert(String::from("OSRELEASE"), String::from("AthenaOS-0.0.1"));
        entries.insert(String::from("PAGESIZE"), String::from("4096"));
        entries.insert(String::from("CRASHTIME"), String::from("0"));
        Self { entries }
    }

    pub fn set(&mut self, key: &str, value: &str) {
        self.entries.insert(String::from(key), String::from(value));
    }

    pub fn set_symbol(&mut self, name: &str, addr: u64) {
        use alloc::format;
        self.entries
            .insert(format!("SYMBOL({})", name), format!("{:#x}", addr));
    }

    pub fn set_offset(&mut self, struct_name: &str, field: &str, offset: u64) {
        use alloc::format;
        self.entries.insert(
            format!("OFFSET({}.{})", struct_name, field),
            format!("{}", offset),
        );
    }

    pub fn set_size(&mut self, struct_name: &str, size: u64) {
        use alloc::format;
        self.entries
            .insert(format!("SIZE({})", struct_name), format!("{}", size));
    }

    pub fn set_length(&mut self, name: &str, len: u64) {
        use alloc::format;
        self.entries
            .insert(format!("LENGTH({})", name), format!("{}", len));
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        for (k, v) in &self.entries {
            buf.extend_from_slice(k.as_bytes());
            buf.push(b'=');
            buf.extend_from_slice(v.as_bytes());
            buf.push(b'\n');
        }
        buf
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 6. Vmcore ELF Format
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ProgramHeaderType {
    Null = 0,
    Load = 1,
    Note = 4,
}

#[derive(Debug, Clone)]
pub struct VmcorePhdr {
    pub p_type: ProgramHeaderType,
    pub p_flags: u32,
    pub p_offset: u64,
    pub p_vaddr: u64,
    pub p_paddr: u64,
    pub p_filesz: u64,
    pub p_memsz: u64,
    pub p_align: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ElfNoteType {
    Prstatus = 1,
    Prpsinfo = 3,
    Vmcoreinfo = 0x52414545, // "RAEE"
}

#[derive(Debug, Clone)]
pub struct ElfNote {
    pub name: String,
    pub note_type: u32,
    pub desc: Vec<u8>,
}

impl ElfNote {
    pub fn vmcoreinfo(data: &[u8]) -> Self {
        Self {
            name: String::from("VMCOREINFO"),
            note_type: ElfNoteType::Vmcoreinfo as u32,
            desc: Vec::from(data),
        }
    }

    pub fn prstatus(cpu_id: u32, regs: &CpuRegisters) -> Self {
        let mut desc = Vec::new();
        desc.extend_from_slice(&regs.rax.to_le_bytes());
        desc.extend_from_slice(&regs.rbx.to_le_bytes());
        desc.extend_from_slice(&regs.rcx.to_le_bytes());
        desc.extend_from_slice(&regs.rdx.to_le_bytes());
        desc.extend_from_slice(&regs.rsi.to_le_bytes());
        desc.extend_from_slice(&regs.rdi.to_le_bytes());
        desc.extend_from_slice(&regs.rbp.to_le_bytes());
        desc.extend_from_slice(&regs.rsp.to_le_bytes());
        desc.extend_from_slice(&regs.r8.to_le_bytes());
        desc.extend_from_slice(&regs.r9.to_le_bytes());
        desc.extend_from_slice(&regs.r10.to_le_bytes());
        desc.extend_from_slice(&regs.r11.to_le_bytes());
        desc.extend_from_slice(&regs.r12.to_le_bytes());
        desc.extend_from_slice(&regs.r13.to_le_bytes());
        desc.extend_from_slice(&regs.r14.to_le_bytes());
        desc.extend_from_slice(&regs.r15.to_le_bytes());
        desc.extend_from_slice(&regs.rip.to_le_bytes());
        desc.extend_from_slice(&regs.rflags.to_le_bytes());
        desc.extend_from_slice(&regs.cs.to_le_bytes());
        desc.extend_from_slice(&regs.ss.to_le_bytes());
        Self {
            name: alloc::format!("CORE_{}", cpu_id),
            note_type: ElfNoteType::Prstatus as u32,
            desc,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
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
    pub cr3: u64,
}

pub struct VmcoreBuilder {
    pub notes: Vec<ElfNote>,
    pub segments: Vec<VmcorePhdr>,
}

impl VmcoreBuilder {
    pub fn new() -> Self {
        Self {
            notes: Vec::new(),
            segments: Vec::new(),
        }
    }

    pub fn add_note(&mut self, note: ElfNote) {
        self.notes.push(note);
    }

    pub fn add_memory_segment(&mut self, paddr: u64, size: u64, offset: u64) {
        self.segments.push(VmcorePhdr {
            p_type: ProgramHeaderType::Load,
            p_flags: 0x04,
            p_offset: offset,
            p_vaddr: 0,
            p_paddr: paddr,
            p_filesz: size,
            p_memsz: size,
            p_align: 0x1000,
        });
    }

    pub fn segment_count(&self) -> usize {
        self.notes.len() + self.segments.len()
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 7. Makedumpfile Support
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DumpLevel {
    All = 0,
    ExcludeZero = 1,
    ExcludeCache = 2,
    ExcludeCachePriv = 4,
    ExcludeUser = 8,
    ExcludeFree = 16,
}

pub struct PageBitmap {
    pub bitmap: Vec<u64>,
    pub total: u64,
    pub excluded: u64,
    pub page_size: u64,
}

impl PageBitmap {
    pub fn new(total_pages: u64, page_size: u64) -> Self {
        let words = ((total_pages + 63) / 64) as usize;
        let mut bitmap = Vec::with_capacity(words);
        bitmap.resize(words, u64::MAX);
        Self {
            bitmap,
            total: total_pages,
            excluded: 0,
            page_size,
        }
    }

    pub fn exclude_page(&mut self, pfn: u64) {
        let idx = (pfn / 64) as usize;
        let bit = pfn % 64;
        if idx < self.bitmap.len() {
            if (self.bitmap[idx] & (1u64 << bit)) != 0 {
                self.bitmap[idx] &= !(1u64 << bit);
                self.excluded += 1;
            }
        }
    }

    pub fn is_included(&self, pfn: u64) -> bool {
        let idx = (pfn / 64) as usize;
        let bit = pfn % 64;
        if idx < self.bitmap.len() {
            (self.bitmap[idx] & (1u64 << bit)) != 0
        } else {
            false
        }
    }

    pub fn included_pages(&self) -> u64 {
        self.total - self.excluded
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 8. Panic Notifier Chain
// ───────────────────────────────────────────────────────────────────────────────

pub type PanicCallbackFn = fn(&PanicInfo);

#[derive(Debug, Clone)]
pub struct PanicInfo {
    pub message: String,
    pub cpu_id: u32,
    pub timestamp: u64,
    pub registers: CpuRegisters,
}

struct PanicNotifier {
    callback: PanicCallbackFn,
    priority: i32,
    name: String,
}

pub struct PanicNotifierChain {
    notifiers: Vec<PanicNotifier>,
}

impl PanicNotifierChain {
    pub fn new() -> Self {
        Self {
            notifiers: Vec::new(),
        }
    }

    pub fn register(&mut self, name: &str, callback: PanicCallbackFn, priority: i32) {
        let entry = PanicNotifier {
            callback,
            priority,
            name: String::from(name),
        };
        let pos = self
            .notifiers
            .iter()
            .position(|n| n.priority < priority)
            .unwrap_or(self.notifiers.len());
        self.notifiers.insert(pos, entry);
    }

    pub fn unregister(&mut self, name: &str) {
        self.notifiers.retain(|n| n.name != name);
    }

    pub fn notify_all(&self, info: &PanicInfo) {
        for notifier in &self.notifiers {
            (notifier.callback)(info);
        }
    }

    pub fn count(&self) -> usize {
        self.notifiers.len()
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 9. Memory Reservation (CMA / firmware map)
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryRegionType {
    Usable,
    Reserved,
    AcpiReclaimable,
    AcpiNvs,
    BadMemory,
    CrashKernel,
    CmaReserved,
}

#[derive(Debug, Clone)]
pub struct ReservedRegion {
    pub start: u64,
    pub size: u64,
    pub region_type: MemoryRegionType,
    pub name: String,
}

pub struct MemoryReservation {
    pub regions: Vec<ReservedRegion>,
    pub crash_region: Option<CrashKernelRegion>,
    pub cma_total: u64,
    pub cma_allocated: u64,
}

impl MemoryReservation {
    pub fn new() -> Self {
        Self {
            regions: Vec::new(),
            crash_region: None,
            cma_total: 0,
            cma_allocated: 0,
        }
    }

    pub fn reserve(&mut self, start: u64, size: u64, rtype: MemoryRegionType, name: &str) -> bool {
        for r in &self.regions {
            if start < r.start + r.size && start + size > r.start {
                return false;
            }
        }
        self.regions.push(ReservedRegion {
            start,
            size,
            region_type: rtype,
            name: String::from(name),
        });
        true
    }

    pub fn reserve_crash_kernel(&mut self, size: u64, offset: Option<u64>) -> bool {
        let start = offset.unwrap_or(0x1000_0000);
        if !self.reserve(start, size, MemoryRegionType::CrashKernel, "crashkernel") {
            return false;
        }
        self.crash_region = Some(CrashKernelRegion {
            start,
            size,
            in_use: false,
        });
        true
    }

    pub fn cma_alloc(&mut self, size: u64) -> Option<u64> {
        if self.cma_allocated + size > self.cma_total {
            return None;
        }
        let addr = 0x2000_0000 + self.cma_allocated;
        self.cma_allocated += size;
        self.reserve(addr, size, MemoryRegionType::CmaReserved, "cma");
        Some(addr)
    }

    pub fn cma_free(&mut self, addr: u64, size: u64) {
        self.regions
            .retain(|r| !(r.start == addr && r.size == size));
        if self.cma_allocated >= size {
            self.cma_allocated -= size;
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 10. Kexec System (ties everything together)
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KexecState {
    Idle,
    Loaded,
    Executing,
    CrashLoaded,
}

pub struct KexecSystem {
    pub state: KexecState,
    pub segments: Vec<KexecSegment>,
    pub entry_point: u64,
    pub format: KernelFormat,
    pub cmdline: String,
    pub purgatory: Purgatory,
    pub vmcoreinfo: VmcoreInfo,
    pub vmcore: VmcoreBuilder,
    pub panic_chain: PanicNotifierChain,
    pub reservation: MemoryReservation,
    pub crash_state: KexecState,
    pub crash_segs: Vec<KexecSegment>,
    pub crash_entry: u64,
}

impl KexecSystem {
    pub fn new() -> Self {
        Self {
            state: KexecState::Idle,
            segments: Vec::new(),
            entry_point: 0,
            format: KernelFormat::Unknown,
            cmdline: String::new(),
            purgatory: Purgatory::new(),
            vmcoreinfo: VmcoreInfo::new(),
            vmcore: VmcoreBuilder::new(),
            panic_chain: PanicNotifierChain::new(),
            reservation: MemoryReservation::new(),
            crash_state: KexecState::Idle,
            crash_segs: Vec::new(),
            crash_entry: 0,
        }
    }

    pub fn kexec_load(
        &mut self,
        kernel: &[u8],
        initrd: Option<&[u8]>,
        cmdline: &str,
    ) -> Result<(), &'static str> {
        let format = KernelFormat::detect(kernel);
        if format == KernelFormat::Unknown {
            return Err("unknown kernel format");
        }

        self.segments.clear();
        let base_addr: u64 = 0x10_0000;

        match format {
            KernelFormat::Elf64 => {
                let hdr = ElfHeader64::parse(kernel)?;
                self.entry_point = hdr.entry_point;
                self.segments
                    .push(KexecSegment::new(SegmentType::Text, kernel, base_addr));
            }
            KernelFormat::BzImage => {
                let _hdr = BzImageHeader::parse(kernel)?;
                self.entry_point = base_addr;
                self.segments
                    .push(KexecSegment::new(SegmentType::Text, kernel, base_addr));
            }
            _ => {
                self.entry_point = base_addr;
                self.segments
                    .push(KexecSegment::new(SegmentType::Text, kernel, base_addr));
            }
        }

        if let Some(rd) = initrd {
            let initrd_addr = base_addr + (kernel.len() as u64 + 0xFFF) & !0xFFF;
            self.segments
                .push(KexecSegment::new(SegmentType::Initrd, rd, initrd_addr));
        }

        if !cmdline.is_empty() {
            let cmd_addr = 0x9_0000;
            self.segments.push(KexecSegment::new(
                SegmentType::CommandLine,
                cmdline.as_bytes(),
                cmd_addr,
            ));
            self.cmdline = String::from(cmdline);
        }

        for seg in &self.segments {
            self.purgatory.add_segment_check(seg.mem_addr, seg.hash);
        }

        self.format = format;
        self.state = KexecState::Loaded;
        Ok(())
    }

    pub fn kexec_load_crash(&mut self, kernel: &[u8], cmdline: &str) -> Result<(), &'static str> {
        let region = self
            .reservation
            .crash_region
            .ok_or("no crash kernel region reserved")?;
        let format = KernelFormat::detect(kernel);
        if format == KernelFormat::Unknown {
            return Err("unknown kernel format");
        }

        self.crash_segs.clear();
        self.crash_segs
            .push(KexecSegment::new(SegmentType::Text, kernel, region.start));
        if !cmdline.is_empty() {
            let cmd_addr = region.start + region.size - 0x1000;
            self.crash_segs.push(KexecSegment::new(
                SegmentType::CommandLine,
                cmdline.as_bytes(),
                cmd_addr,
            ));
        }
        self.crash_entry = region.start;
        self.crash_state = KexecState::CrashLoaded;
        Ok(())
    }

    pub fn verify_segments(&self) -> bool {
        self.segments.iter().all(|s| s.verify_integrity())
    }

    pub fn kexec_execute(&mut self) -> Result<(), &'static str> {
        if self.state != KexecState::Loaded {
            return Err("no kernel loaded");
        }
        if !self.verify_segments() {
            return Err("segment integrity check failed");
        }
        self.state = KexecState::Executing;
        Ok(())
    }

    pub fn handle_panic(&mut self, info: &PanicInfo) {
        self.panic_chain.notify_all(info);

        let vmcoreinfo_data = self.vmcoreinfo.serialize();
        self.vmcore.add_note(ElfNote::vmcoreinfo(&vmcoreinfo_data));
        self.vmcore
            .add_note(ElfNote::prstatus(info.cpu_id, &info.registers));

        if self.crash_state == KexecState::CrashLoaded {
            self.crash_state = KexecState::Executing;
        }
    }

    pub fn stats(&self) -> KexecStats {
        KexecStats {
            state: self.state,
            crash_state: self.crash_state,
            segment_count: self.segments.len(),
            crash_seg_count: self.crash_segs.len(),
            entry_point: self.entry_point,
            format: self.format,
            panic_notifiers: self.panic_chain.count(),
            reserved_regions: self.reservation.regions.len(),
        }
    }
}

#[derive(Debug)]
pub struct KexecStats {
    pub state: KexecState,
    pub crash_state: KexecState,
    pub segment_count: usize,
    pub crash_seg_count: usize,
    pub entry_point: u64,
    pub format: KernelFormat,
    pub panic_notifiers: usize,
    pub reserved_regions: usize,
}

pub static KEXEC_SYSTEM: Mutex<Option<KexecSystem>> = Mutex::new(None);

pub fn init() {
    let mut sys = KEXEC_SYSTEM.lock();
    *sys = Some(KexecSystem::new());
}
