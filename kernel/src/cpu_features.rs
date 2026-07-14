//! CPU feature detection — milestone M-A prep.
//!
//! `kernelchecklist.md` §M-A "Boots on Athena":
//! > "AMD Zen 4 CPUID feature detection"
//! > "All 12 cores online via SMP"
//!
//! Runs CPUID at boot, fingerprints vendor + family + model + stepping +
//! brand string, and probes for the feature flags we actually depend on:
//! SSE2/SSSE3/SSE4.2 (memcpy/memset), AVX2/AVX-512 (vectorized search
//! index, future), RDRAND/RDSEED (crypto seeds), TSC_DEADLINE (HPET-free
//! timer mode), x2APIC (more than 256 cores), PCID/INVPCID (page-table
//! switch acceleration), SMEP/SMAP/CET (kernel hardening), LAM/UAI (memory
//! tagging — Concept §Security), VMX/SVM (virtualization), and Hybrid
//! Topology (Intel P/E cores, AMD CCX/Node clusters — for SCHED_GAME).
//!
//! Output: `[cpu] vendor=… family=… brand="…" features=…` at boot, plus
//! `/proc/raeen/cpu` for runtime introspection.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::{String, ToString};
use spin::Mutex;

// ── CPUID raw ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default)]
pub struct CpuidResult {
    pub eax: u32,
    pub ebx: u32,
    pub ecx: u32,
    pub edx: u32,
}

pub fn cpuid_raw(leaf: u32, sub: u32) -> CpuidResult {
    let mut r = CpuidResult::default();
    // LLVM reserves RBX for its own use, so we can't name `ebx` directly.
    // Save/restore via the stack — standard CPUID idiom under Rust's
    // inline assembler.
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            tmp = lateout(reg) r.ebx,
            inout("eax") leaf => r.eax,
            inout("ecx") sub  => r.ecx,
            lateout("edx") r.edx,
            options(nostack, preserves_flags),
        );
    }
    r
}

// ── Vendor + identity ──────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vendor {
    Amd,
    Intel,
    Hypervisor,
    Other,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Identity {
    pub family: u32,
    pub model: u32,
    pub stepping: u32,
    pub max_basic: u32,
    pub max_ext: u32,
}

fn read_identity(eax_1: u32, max_basic: u32) -> Identity {
    // Family/model/stepping computation per Intel + AMD vol 2A:
    //   stepping       = bits[0..4]
    //   base_model     = bits[4..8]
    //   base_family    = bits[8..12]
    //   ext_model      = bits[16..20] — added to base_model if base_family in {6, 15}
    //   ext_family     = bits[20..28] — added to base_family if base_family == 15
    let stepping = eax_1 & 0xF;
    let base_model = (eax_1 >> 4) & 0xF;
    let base_family = (eax_1 >> 8) & 0xF;
    let ext_model = (eax_1 >> 16) & 0xF;
    let ext_family = (eax_1 >> 20) & 0xFF;
    let family = if base_family == 0xF {
        base_family + ext_family
    } else {
        base_family
    };
    let model = if base_family == 0xF || base_family == 0x6 {
        (ext_model << 4) | base_model
    } else {
        base_model
    };
    let max_ext = cpuid_raw(0x8000_0000, 0).eax;
    Identity {
        family,
        model,
        stepping,
        max_basic,
        max_ext,
    }
}

// ── Feature flags ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheType {
    Null = 0,
    Data = 1,
    Instruction = 2,
    Unified = 3,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CacheInfo {
    pub level: u8,
    pub cache_type: CacheType,
    pub size_kb: u32,
    pub line_size: u32,
    pub ways: u32,
    pub sets: u32,
    pub partitions: u32,
    pub sharing_count: u32,
}

impl Default for CacheType {
    fn default() -> Self {
        CacheType::Null
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreType {
    Unknown = 0,
    IntelAtom = 0x20, // Efficiency
    IntelCore = 0x40, // Performance
    AmdZen = 0x80,
}

impl Default for CoreType {
    fn default() -> Self {
        CoreType::Unknown
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Topology {
    pub core_type: CoreType,
    pub efficiency_ranking: u8,
    // AMD specific
    pub compute_unit_id: u8,
    pub cores_per_compute_unit: u8,
    pub node_id: u8,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Features {
    // Leaf 1 / EDX
    pub fpu: bool,
    pub tsc: bool,
    pub msr: bool,
    pub apic: bool,
    pub mmx: bool,
    pub sse: bool,
    pub sse2: bool,

    // Leaf 1 / ECX
    pub sse3: bool,
    pub ssse3: bool,
    pub fma: bool,
    pub cmpxchg16b: bool,
    pub sse41: bool,
    pub sse42: bool,
    pub x2apic: bool,
    pub aes: bool,
    pub xsave: bool,
    pub osxsave: bool,
    pub avx: bool,
    pub f16c: bool,
    pub rdrand: bool,
    pub hypervisor: bool,

    // Leaf 7 sub-leaf 0 / EBX
    pub fsgsbase: bool,
    pub smep: bool,
    pub avx2: bool,
    pub bmi1: bool,
    pub bmi2: bool,
    pub erms: bool,
    pub invpcid: bool,
    pub avx512f: bool,
    pub avx512dq: bool,
    pub rdseed: bool,
    pub adx: bool,
    pub smap: bool,
    pub avx512cd: bool,
    pub sha: bool,
    pub avx512bw: bool,
    pub avx512vl: bool,

    // Leaf 7 / ECX (more recent)
    pub avx512vbmi: bool,
    pub umip: bool,
    pub pku: bool,
    pub cet_ss: bool,
    pub la57: bool,
    pub vaes: bool,
    pub vpclmulqdq: bool,

    // Leaf 7 / EDX
    pub cet_ibt: bool,
    pub amx_bf16: bool,
    pub hybrid: bool, // Intel Hybrid processor

    // Leaf 0x8000_0001 / ECX (AMD-style ext)
    pub lahf_lm: bool,
    pub abm_lzcnt: bool,

    // Leaf 0x8000_0001 / EDX
    pub syscall: bool,
    pub nx: bool,
    pub gb_pages: bool,
    pub rdtscp: bool,
    pub lm_long_mode: bool,

    // Hypervisor leaf
    pub hv_kvm: bool,
    pub hv_qemu: bool,

    // Virtualization extensions
    pub vmx: bool,
    pub svm: bool,

    // Memory tagging (Concept §Security)
    pub lam: bool, // Intel Linear Address Masking
    pub uai: bool, // AMD Upper Address Ignore
}

fn read_features(vendor: Vendor, ident: &Identity) -> Features {
    let mut f = Features::default();

    if ident.max_basic >= 1 {
        let r1 = cpuid_raw(1, 0);
        // EDX
        f.fpu = r1.edx & (1 << 0) != 0;
        f.tsc = r1.edx & (1 << 4) != 0;
        f.msr = r1.edx & (1 << 5) != 0;
        f.apic = r1.edx & (1 << 9) != 0;
        f.mmx = r1.edx & (1 << 23) != 0;
        f.sse = r1.edx & (1 << 25) != 0;
        f.sse2 = r1.edx & (1 << 26) != 0;
        // ECX
        f.sse3 = r1.ecx & (1 << 0) != 0;
        f.ssse3 = r1.ecx & (1 << 9) != 0;
        f.fma = r1.ecx & (1 << 12) != 0;
        f.cmpxchg16b = r1.ecx & (1 << 13) != 0;
        f.sse41 = r1.ecx & (1 << 19) != 0;
        f.sse42 = r1.ecx & (1 << 20) != 0;
        f.x2apic = r1.ecx & (1 << 21) != 0;
        f.aes = r1.ecx & (1 << 25) != 0;
        f.xsave = r1.ecx & (1 << 26) != 0;
        f.osxsave = r1.ecx & (1 << 27) != 0;
        f.avx = r1.ecx & (1 << 28) != 0;
        f.f16c = r1.ecx & (1 << 29) != 0;
        f.rdrand = r1.ecx & (1 << 30) != 0;
        f.hypervisor = r1.ecx & (1 << 31) != 0;
        f.vmx = matches!(vendor, Vendor::Intel) && (r1.ecx & (1 << 5) != 0);
    }

    if ident.max_basic >= 7 {
        let r7_0 = cpuid_raw(7, 0);
        // EBX
        f.fsgsbase = r7_0.ebx & (1 << 0) != 0;
        f.smep = r7_0.ebx & (1 << 7) != 0;
        f.bmi1 = r7_0.ebx & (1 << 3) != 0;
        f.avx2 = r7_0.ebx & (1 << 5) != 0;
        f.bmi2 = r7_0.ebx & (1 << 8) != 0;
        f.erms = r7_0.ebx & (1 << 9) != 0;
        f.invpcid = r7_0.ebx & (1 << 10) != 0;
        f.avx512f = r7_0.ebx & (1 << 16) != 0;
        f.avx512dq = r7_0.ebx & (1 << 17) != 0;
        f.rdseed = r7_0.ebx & (1 << 18) != 0;
        f.adx = r7_0.ebx & (1 << 19) != 0;
        f.smap = r7_0.ebx & (1 << 20) != 0;
        f.avx512cd = r7_0.ebx & (1 << 28) != 0;
        f.sha = r7_0.ebx & (1 << 29) != 0;
        f.avx512bw = r7_0.ebx & (1 << 30) != 0;
        f.avx512vl = r7_0.ebx & (1 << 31) != 0;
        // ECX
        f.avx512vbmi = r7_0.ecx & (1 << 1) != 0;
        f.umip = r7_0.ecx & (1 << 2) != 0;
        f.pku = r7_0.ecx & (1 << 3) != 0;
        f.cet_ss = r7_0.ecx & (1 << 7) != 0;
        f.la57 = r7_0.ecx & (1 << 16) != 0;
        f.vaes = r7_0.ecx & (1 << 9) != 0;
        f.vpclmulqdq = r7_0.ecx & (1 << 10) != 0;
        // EDX
        f.cet_ibt = r7_0.edx & (1 << 20) != 0;
        f.amx_bf16 = r7_0.edx & (1 << 22) != 0;
        f.hybrid = r7_0.edx & (1 << 15) != 0;

        // Sub-leaf 1 ECX for LAM (Intel) — bit 6
        let r7_1 = cpuid_raw(7, 1);
        f.lam = matches!(vendor, Vendor::Intel) && (r7_1.eax & (1 << 26) != 0);
    }

    if ident.max_ext >= 0x8000_0001 {
        let re1 = cpuid_raw(0x8000_0001, 0);
        f.lahf_lm = re1.ecx & (1 << 0) != 0;
        f.abm_lzcnt = re1.ecx & (1 << 5) != 0;
        f.svm = matches!(vendor, Vendor::Amd) && (re1.ecx & (1 << 2) != 0);
        f.syscall = re1.edx & (1 << 11) != 0;
        f.nx = re1.edx & (1 << 20) != 0;
        f.gb_pages = re1.edx & (1 << 26) != 0;
        f.rdtscp = re1.edx & (1 << 27) != 0;
        f.lm_long_mode = re1.edx & (1 << 29) != 0;
    }

    // AMD UAI lives in 0x8000_0008 EBX bit 2 on Zen 4+.
    if matches!(vendor, Vendor::Amd) && ident.max_ext >= 0x8000_0008 {
        let re8 = cpuid_raw(0x8000_0008, 0);
        f.uai = re8.ebx & (1 << 2) != 0;
    }

    // Hypervisor identification (only meaningful if f.hypervisor)
    if f.hypervisor {
        // Standard hypervisor leaves live at 0x4000_0000..0x4000_00FF
        let hv = cpuid_raw(0x4000_0000, 0);
        // Vendor in EBX/ECX/EDX as 12-char ASCII.
        let mut bytes = [0u8; 12];
        bytes[0..4].copy_from_slice(&hv.ebx.to_le_bytes());
        bytes[4..8].copy_from_slice(&hv.ecx.to_le_bytes());
        bytes[8..12].copy_from_slice(&hv.edx.to_le_bytes());
        let s = core::str::from_utf8(&bytes).unwrap_or("");
        f.hv_kvm = s.starts_with("KVMKVMKVM");
        f.hv_qemu = s.starts_with("TCGTCGTCG") || s.contains("QEMU");
    }

    f
}

fn read_topology(vendor: Vendor, ident: &Identity, features: &Features) -> Topology {
    let mut t = Topology::default();

    match vendor {
        Vendor::Intel => {
            if features.hybrid && ident.max_basic >= 0x1A {
                let r1a = cpuid_raw(0x1A, 0);
                let core_type_raw = (r1a.eax >> 24) & 0xFF;
                t.core_type = match core_type_raw {
                    0x20 => CoreType::IntelAtom,
                    0x40 => CoreType::IntelCore,
                    _ => CoreType::Unknown,
                };
                t.efficiency_ranking = (r1a.eax & 0xFF) as u8;
            }
        }
        Vendor::Amd => {
            t.core_type = CoreType::AmdZen;
            if ident.max_ext >= 0x8000_001E {
                let re1e = cpuid_raw(0x8000_001E, 0);
                t.compute_unit_id = (re1e.ebx & 0xFF) as u8;
                t.cores_per_compute_unit = (((re1e.ebx >> 8) & 0xFF) + 1) as u8;
                t.node_id = (re1e.ecx & 0xFF) as u8;
            }
        }
        _ => {}
    }

    t
}

fn read_cache_topology(vendor: Vendor, ident: &Identity) -> alloc::vec::Vec<CacheInfo> {
    let mut caches = alloc::vec::Vec::new();
    let (leaf, max_sub) = match vendor {
        Vendor::Intel => (0x4, 32), // Intel: Deterministic Cache Parameters
        Vendor::Amd if ident.max_ext >= 0x8000_001D => (0x8000_001D, 32), // AMD: Cache Topology
        _ => return caches,
    };

    for sub in 0..max_sub {
        let r = cpuid_raw(leaf, sub);
        let cache_type_raw = r.eax & 0x1F;
        if cache_type_raw == 0 {
            break;
        } // Null cache type: end of list

        let cache_type = match cache_type_raw {
            1 => CacheType::Data,
            2 => CacheType::Instruction,
            3 => CacheType::Unified,
            _ => CacheType::Null,
        };

        let level = ((r.eax >> 5) & 0x7) as u8;
        let sharing_count = ((r.eax >> 14) & 0xFFF) + 1;
        let line_size = (r.ebx & 0xFFF) + 1;
        let partitions = ((r.ebx >> 12) & 0x3FF) + 1;
        let ways = ((r.ebx >> 22) & 0x3FF) + 1;
        let sets = r.ecx + 1;

        let size_bytes = ways * partitions * line_size * sets;
        let size_kb = size_bytes / 1024;

        caches.push(CacheInfo {
            level,
            cache_type,
            size_kb,
            line_size,
            ways,
            sets,
            partitions,
            sharing_count,
        });
    }

    caches
}

fn read_vendor() -> (Vendor, String, u32) {
    let r = cpuid_raw(0, 0);
    let mut bytes = [0u8; 12];
    bytes[0..4].copy_from_slice(&r.ebx.to_le_bytes());
    bytes[4..8].copy_from_slice(&r.edx.to_le_bytes());
    bytes[8..12].copy_from_slice(&r.ecx.to_le_bytes());
    let s = core::str::from_utf8(&bytes).unwrap_or("").to_string();
    let vendor = match s.as_str() {
        "GenuineIntel" => Vendor::Intel,
        "AuthenticAMD" => Vendor::Amd,
        _ => Vendor::Other,
    };
    (vendor, s, r.eax)
}

fn read_brand_string() -> String {
    // 0x8000_0002..4 return 48 ASCII bytes.
    let max_ext = cpuid_raw(0x8000_0000, 0).eax;
    if max_ext < 0x8000_0004 {
        return String::new();
    }
    let mut bytes = [0u8; 48];
    for (i, leaf) in [0x8000_0002, 0x8000_0003, 0x8000_0004].iter().enumerate() {
        let r = cpuid_raw(*leaf, 0);
        let off = i * 16;
        bytes[off..off + 4].copy_from_slice(&r.eax.to_le_bytes());
        bytes[off + 4..off + 8].copy_from_slice(&r.ebx.to_le_bytes());
        bytes[off + 8..off + 12].copy_from_slice(&r.ecx.to_le_bytes());
        bytes[off + 12..off + 16].copy_from_slice(&r.edx.to_le_bytes());
    }
    // Trim leading + trailing whitespace and stop at the first NUL.
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(48);
    core::str::from_utf8(&bytes[..end])
        .unwrap_or("")
        .trim()
        .to_string()
}

// ── Cached snapshot ────────────────────────────────────────────────────

pub struct CpuInfo {
    pub vendor: Vendor,
    pub vendor_str: String,
    pub identity: Identity,
    pub brand: String,
    pub features: Features,
    pub topology: Topology,
    pub cache_topology: alloc::vec::Vec<CacheInfo>,
}

static INFO: Mutex<Option<CpuInfo>> = Mutex::new(None);

fn build_snapshot() -> CpuInfo {
    let (vendor, vendor_str, max_basic) = read_vendor();
    let r1 = cpuid_raw(1, 0);
    let identity = read_identity(r1.eax, max_basic);
    let features = read_features(vendor, &identity);
    let topology = read_topology(vendor, &identity, &features);
    let brand = read_brand_string();
    let cache_topology = read_cache_topology(vendor, &identity);
    CpuInfo {
        vendor,
        vendor_str,
        identity,
        brand,
        features,
        topology,
        cache_topology,
    }
}

/// Enable SSE/SSE2 execution on the current CPU.
///
/// The kernel itself is built soft-float (no SSE in kernel code), so SSE was
/// never turned on — but **userspace** code (relibc, compiled with SSE) and the
/// `fxsave64` in `switch_context` need it. Without this, the very first SSE
/// instruction in a userspace `_start` (`ldmxcsr`) raises #UD.
///
/// Sets CR0.MP=1, CR0.EM=0, CR4.OSFXSR=1, CR4.OSXMMEXCPT=1. Must run early on
/// every CPU (BSP + each AP) before any task can reach userspace.
#[inline]
pub fn enable_sse() {
    use core::arch::asm;
    unsafe {
        let mut cr0: u64;
        asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
        cr0 &= !(1u64 << 2); // EM = 0 (no x87 emulation)
        cr0 |= 1u64 << 1; // MP = 1 (monitor coprocessor)
        asm!("mov cr0, {}", in(reg) cr0, options(nomem, nostack, preserves_flags));

        let mut cr4: u64;
        asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
        cr4 |= (1u64 << 9) | (1u64 << 10); // OSFXSR | OSXMMEXCPT
        asm!("mov cr4, {}", in(reg) cr4, options(nomem, nostack, preserves_flags));
    }
}

/// True if the CPU advertises AES-NI — `CPUID.1:ECX[bit 25]`. Gates the
/// hardware AES fast path in `crypto` (AESENC/AESDEC).
pub fn aesni_supported() -> bool {
    core::arch::x86_64::__cpuid(1).ecx & (1 << 25) != 0
}

/// True if the CPU advertises the RDRAND instruction — `CPUID.1:ECX[bit 30]`.
/// Gates the hardware CSPRNG source used to seed `crypto`'s RNG.
pub fn rdrand_supported() -> bool {
    core::arch::x86_64::__cpuid(1).ecx & (1 << 30) != 0
}

/// True if the CPU advertises the RDSEED instruction —
/// `CPUID.(EAX=7,ECX=0):EBX[bit 18]`. RDSEED is the true-random (full-entropy)
/// seed source; preferred over RDRAND for seeding a DRBG.
pub fn rdseed_supported() -> bool {
    core::arch::x86_64::__cpuid_count(7, 0).ebx & (1 << 18) != 0
}

/// True if the CPU advertises SMEP — `CPUID.(EAX=7,ECX=0):EBX[bit 7]`.
pub fn smep_supported() -> bool {
    core::arch::x86_64::__cpuid_count(7, 0).ebx & (1 << 7) != 0
}

/// True if `CR4.SMEP` (bit 20) is set on the CURRENT CPU. A read-back of the
/// hardware bit — not a status flag, so it cannot lie.
pub fn smep_active() -> bool {
    let cr4: u64;
    unsafe {
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
    }
    cr4 & (1u64 << 20) != 0
}

/// Enable hardware SMEP (Supervisor Mode Execution Prevention) on the CURRENT
/// CPU when supported: set `CR4.SMEP` so any ring-0 attempt to EXECUTE a
/// user-accessible page faults — hardware ret2usr prevention, zero runtime cost.
/// `CR4` is per-CPU, so this must run on the BSP (from the hardening init) AND on
/// every AP (from `ap_entry`). Safe because the kernel never executes user
/// pages. Returns whether SMEP is now active on this CPU.
///
/// Concept §"Security by default, not by friction": a REAL hardware trap,
/// replacing the previously software-simulated SMEP status.
pub fn enable_smep() -> bool {
    if !smep_supported() {
        return false;
    }
    unsafe {
        let mut cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
        cr4 |= 1u64 << 20; // CR4.SMEP
        core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nomem, nostack, preserves_flags));
    }
    smep_active()
}

/// True if the CPU advertises UMIP — `CPUID.(EAX=7,ECX=0):ECX[bit 2]`.
pub fn umip_supported() -> bool {
    core::arch::x86_64::__cpuid_count(7, 0).ecx & (1 << 2) != 0
}

/// True if `CR4.UMIP` (bit 11) is set on the CURRENT CPU. A read-back of the
/// hardware bit — not a status flag, so it cannot lie.
pub fn umip_active() -> bool {
    let cr4: u64;
    unsafe {
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
    }
    cr4 & (1u64 << 11) != 0
}

/// Enable hardware UMIP (User-Mode Instruction Prevention) on the CURRENT CPU
/// when supported: set `CR4.UMIP` (bit 11) so the five descriptor-table /
/// machine-status instructions `SGDT`, `SIDT`, `SLDT`, `STR`, `SMSW` raise
/// `#GP` when executed at CPL > 0. Those instructions are unprivileged by
/// default and leak the linear addresses of the GDT / IDT / LDT / TSS to
/// userspace — a KASLR-defeat information leak (the classic ret2dir / SIDT
/// de-randomization primitive). Blocking them costs nothing and breaks no
/// legitimate userspace (RaeenOS apps never need raw descriptor-table reads).
/// `CR4` is per-CPU: BSP via the hardening init, every AP via `ap_entry`.
///
/// Concept §"Security by default, not by friction": a REAL hardware trap that
/// closes a kernel-address info-leak, in the same vein as SMEP/SMAP. Returns
/// whether UMIP is now active on this CPU.
pub fn enable_umip() -> bool {
    if !umip_supported() {
        return false;
    }
    unsafe {
        let mut cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
        cr4 |= 1u64 << 11; // CR4.UMIP
        core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nomem, nostack, preserves_flags));
    }
    umip_active()
}

/// R10 FAIL-able hardening smoketest (BSP): if the CPU advertises UMIP then
/// `CR4.UMIP` MUST be set (we enable it during CPU bring-up); if the CPU lacks
/// UMIP we honestly skip. A regression that leaves the bit clear on a
/// UMIP-capable CPU prints FAIL.
pub fn run_umip_smoketest() {
    let supported = umip_supported();
    let active = umip_active();
    let pass = !supported || active;
    crate::serial_println!(
        "[cpu-harden] UMIP: cpuid_supported={} cr4_active={} (blocks SGDT/SIDT/SLDT/STR/SMSW @CPL>0) -> {}",
        supported,
        active,
        if pass { "PASS" } else { "FAIL" }
    );
}

/// True if the CPU advertises SMAP — `CPUID.(EAX=7,ECX=0):EBX[bit 20]`.
pub fn smap_supported() -> bool {
    core::arch::x86_64::__cpuid_count(7, 0).ebx & (1 << 20) != 0
}

/// True if `CR4.SMAP` (bit 21) is set on the CURRENT CPU. A read-back of the
/// hardware bit — not a status flag, so it cannot lie.
pub fn smap_active() -> bool {
    let cr4: u64;
    unsafe {
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
    }
    cr4 & (1u64 << 21) != 0
}

/// Enable hardware SMAP (Supervisor Mode Access Prevention) on the CURRENT
/// CPU when supported: with `CR4.SMAP` set, any ring-0 READ or WRITE of a
/// user-accessible (U=1) page faults unless `EFLAGS.AC` is set — so the only
/// legal kernel touches of user memory are the `stac`/`clac`-bracketed copy
/// stubs in `extable` (which every `uaccess` helper routes through). `CR4` is
/// per-CPU: BSP via the hardening init, every AP via `ap_entry`.
///
/// Ordering contract: the extable stub selector (`extable::SMAP_ON`) is armed
/// BEFORE the CR4 bit is set, so there is no window where a user copy runs the
/// plain (non-stac) stub on a SMAP-active CPU and eats a spurious EFAULT.
/// `stac`/`clac` are legal whenever CPUID advertises SMAP regardless of CR4,
/// so arming the stub early on not-yet-SMAP CPUs is harmless.
///
/// Concept §"Security by default, not by friction": a REAL hardware trap —
/// kernel dereference of a user pointer outside the validated chokepoint is
/// now a fault, not a silent info-leak/privesc primitive.
pub fn enable_smap() -> bool {
    if !smap_supported() {
        return false;
    }
    // Arm the stac/clac copy stubs FIRST (see ordering contract above).
    crate::extable::arm_smap_stubs();
    unsafe {
        let mut cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
        cr4 |= 1u64 << 21; // CR4.SMAP
        core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nomem, nostack, preserves_flags));
    }
    smap_active()
}

/// R10 FAIL-able hardening smoketest (BSP): if the CPU advertises SMEP then
/// `CR4.SMEP` MUST be set (we enable it during CPU bring-up); if the CPU lacks
/// SMEP we honestly skip. A regression that leaves the bit clear on a
/// SMEP-capable CPU prints FAIL.
pub fn run_smep_smoketest() {
    let supported = smep_supported();
    let active = smep_active();
    let pass = !supported || active;
    crate::serial_println!(
        "[cpu-harden] SMEP: cpuid_supported={} cr4_active={} (real HW ret2usr guard) -> {}",
        supported,
        active,
        if pass { "PASS" } else { "FAIL" }
    );
}

/// R10 FAIL-able hardening smoketest (BSP): prove REAL hardware SMAP actually
/// traps a supervisor access to a user page — not just that the CR4 bit is set.
/// Maps a throwaway USER-accessible page, then:
///  1. `copy_to_user` (the stac/clac chokepoint) must SUCCEED writing a sentinel
///     — the legal path stays open.
///  2. the PLAIN (non-stac) probe reading the same page must FAULT→`Err` — that
///     observable fault IS the SMAP trap (with AC clear, a supervisor read of a
///     U=1 page #PFs; the extable fixup turns it into `Err` instead of #DF).
///  3. `copy_from_user` reads the sentinel back correctly (stac path again).
/// If SMAP is unsupported (older CPU / TCG without `+smap`) we honestly skip the
/// behavioral half and only assert the read-back CR4 state. A regression that
/// leaves CR4.SMAP clear, or lets the non-stac read SUCCEED, prints FAIL.
pub fn run_smap_smoketest() {
    let supported = smap_supported();
    let active = smap_active();
    // The behavioral probe only means anything when the bit is actually live.
    if !supported || !active {
        let pass = !supported; // supported-but-inactive is a real regression
        crate::serial_println!(
            "[cpu-harden] SMAP: cpuid_supported={} cr4_active={} behavioral=skipped -> {}",
            supported,
            active,
            if pass { "PASS" } else { "FAIL" }
        );
        return;
    }

    use x86_64::registers::control::Cr3;
    use x86_64::structures::paging::{Page, PageTableFlags};
    use x86_64::VirtAddr;

    // A user VA that is free at Tier-4 boot (no task spawned yet).
    const TEST_VA: u64 = 0x0000_5000_0000;
    let (pml4, _) = Cr3::read();
    let page = Page::containing_address(VirtAddr::new(TEST_VA));

    let frame = match crate::memory::allocate_frame() {
        Some(f) => f,
        None => {
            crate::serial_println!(
                "[cpu-harden] SMAP: cpuid_supported=true cr4_active=true behavioral=no_frame -> FAIL"
            );
            return;
        }
    };
    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::USER_ACCESSIBLE
        | PageTableFlags::NO_EXECUTE;
    unsafe {
        crate::memory::map_page_in_pml4(pml4, page, frame, flags);
        // The leaf PTE alone is not enough: the CPU ANDs the U bit across ALL
        // paging levels, so the intermediate tables map_to created (supervisor
        // by default) would leave TEST_VA effectively SUPERVISOR — and SMAP
        // only traps supervisor access to USER pages, so the probe would NOT
        // fault on real hardware (TCG only checks the leaf, which masked this).
        // Propagate USER_ACCESSIBLE up every level so the page is genuinely
        // ring-3-reachable and the SMAP trap is exercised for real.
        crate::memory::make_user_accessible(VirtAddr::new(TEST_VA), 0x1000);
    }

    // 1. stac path writes the sentinel (must succeed — chokepoint stays open).
    let sentinel: [u8; 8] = [0xA5, 0x5A, 0x11, 0x22, 0x33, 0x44, 0xDE, 0xAD];
    let write_ok = crate::uaccess::copy_to_user(TEST_VA, &sentinel).is_ok();

    // 2. plain (non-stac) supervisor read MUST fault under SMAP → Err.
    let mut scratch = [0u8; 8];
    let no_stac_faulted = unsafe {
        crate::extable::copy_user_no_stac_probe(
            TEST_VA as *const u8,
            scratch.as_mut_ptr(),
            scratch.len(),
        )
    }
    .is_err();

    // 3. stac path reads it back correctly.
    let readback = crate::uaccess::copy_from_user(TEST_VA, 8).unwrap_or_default();
    let readback_ok = readback == sentinel;

    unsafe {
        crate::memory::unmap_page_in_pml4(pml4, page);
    }
    crate::memory::deallocate_frame(frame);

    let pass = write_ok && no_stac_faulted && readback_ok;
    crate::serial_println!(
        "[cpu-harden] SMAP: cpuid_supported=true cr4_active=true stac_write={} nonstac_read_faults={} stac_readback={} -> {}",
        write_ok,
        no_stac_faulted,
        readback_ok,
        if pass { "PASS" } else { "FAIL" }
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Spectre / branch-target speculation controls (IA32_SPEC_CTRL, IA32_PRED_CMD)
//
// Meltdown/Spectre-class transient-execution defenses that live in MSRs, not
// CR4. IA32_SPEC_CTRL (0x48) holds three PERSISTENT, set-once bits:
//   bit 0  IBRS  — Indirect Branch Restricted Speculation
//   bit 1  STIBP — Single-Thread Indirect Branch Predictors (SMT isolation)
//   bit 2  SSBD  — Speculative Store Bypass Disable (Spectre v4)
// These are set-once (no per-privilege-transition rewrite), so steady-state
// cost is near zero — the right choice for a latency-sensitive gaming kernel
// (we deliberately do NOT rewrite IBRS on every kernel entry the legacy way).
// IA32_PRED_CMD (0x49) bit 0 = IBPB, a one-shot barrier issued on a security-
// domain switch (a scheduler concern, not a persistent state).
//
// Support is advertised on DIFFERENT CPUID leaves per vendor:
//   Intel : CPUID.(7,0):EDX          bit 26 IBRS/IBPB, 27 STIBP, 31 SSBD
//   AMD   : CPUID.(8000_0008):EBX    bit 12 IBPB, 14 IBRS, 15 STIBP, 24 SSBD
// Every write routes through msr::wrmsr_safe, so an emulator that does not back
// SPEC_CTRL (TCG without +spec-ctrl) reports the MSR absent instead of #GP-
// crashing the boot, and the value is READ BACK — the status cannot lie. This
// replaces the previously software-simulated Spectre status (flags set with no
// MSR behind them), in the same honesty vein as the SMEP/SMAP/UMIP work.
// ───────────────────────────────────────────────────────────────────────────

const IA32_SPEC_CTRL: u32 = 0x48;
const IA32_PRED_CMD: u32 = 0x49;
const SPEC_CTRL_IBRS: u64 = 1 << 0;
const SPEC_CTRL_STIBP: u64 = 1 << 1;
const SPEC_CTRL_SSBD: u64 = 1 << 2;

/// Which branch-speculation controls THIS CPU advertises (per-vendor CPUID).
#[derive(Clone, Copy, Debug, Default)]
pub struct SpecCtrlSupport {
    pub ibrs: bool,
    pub stibp: bool,
    pub ssbd: bool,
    pub ibpb: bool,
}

impl SpecCtrlSupport {
    /// True if the CPU advertises any persistent SPEC_CTRL bit.
    pub fn any_persistent(&self) -> bool {
        self.ibrs || self.stibp || self.ssbd
    }
}

/// Per-vendor CPUID probe of the branch-speculation control features.
pub fn spec_ctrl_support() -> SpecCtrlSupport {
    match crate::msr::cpu_vendor() {
        crate::msr::CpuVendor::Intel => {
            let edx = core::arch::x86_64::__cpuid_count(7, 0).edx;
            SpecCtrlSupport {
                ibrs: edx & (1 << 26) != 0,
                stibp: edx & (1 << 27) != 0,
                ssbd: edx & (1 << 31) != 0,
                ibpb: edx & (1 << 26) != 0, // Intel folds IBPB into the IBRS bit
            }
        }
        crate::msr::CpuVendor::Amd => {
            // Leaf 0x8000_0008 EBX — only meaningful if the max ext leaf covers it.
            let max_ext = core::arch::x86_64::__cpuid(0x8000_0000).eax;
            if max_ext < 0x8000_0008 {
                return SpecCtrlSupport::default();
            }
            let ebx = core::arch::x86_64::__cpuid(0x8000_0008).ebx;
            SpecCtrlSupport {
                ibpb: ebx & (1 << 12) != 0,
                ibrs: ebx & (1 << 14) != 0,
                stibp: ebx & (1 << 15) != 0,
                ssbd: ebx & (1 << 24) != 0,
            }
        }
        crate::msr::CpuVendor::Other => SpecCtrlSupport::default(),
    }
}

/// Current IA32_SPEC_CTRL value on THIS CPU, or `None` if the MSR is absent
/// (e.g. under TCG without `+spec-ctrl`). A hardware read-back — cannot lie.
pub fn spec_ctrl_read() -> Option<u64> {
    unsafe { crate::msr::rdmsr_safe(IA32_SPEC_CTRL) }
}

/// Program the persistent branch-speculation defenses (IBRS + STIBP + SSBD)
/// into IA32_SPEC_CTRL on the CURRENT CPU, for whichever bits CPUID advertises.
/// Set-once bits, so ~zero steady-state cost. `#GP`-safe via `wrmsr_safe` (an
/// emulator lacking the MSR reports unsupported rather than faulting). MSR
/// state is per-CPU: BSP via the hardening init, every AP via `ap_entry`.
/// Returns the value actually READ BACK after the write (`0` if the MSR is
/// absent) so callers can record honest, verified state.
///
/// Concept §"Security by default, not by friction": a REAL branch-speculation
/// hardware defense, replacing the previously software-simulated status.
pub fn enable_spec_ctrl() -> u64 {
    let sup = spec_ctrl_support();
    // If SPEC_CTRL is not readable, the MSR does not exist here — nothing to do.
    let cur = match spec_ctrl_read() {
        Some(v) => v,
        None => return 0,
    };
    let mut want = cur;
    if sup.ibrs {
        want |= SPEC_CTRL_IBRS;
    }
    if sup.stibp {
        want |= SPEC_CTRL_STIBP;
    }
    if sup.ssbd {
        want |= SPEC_CTRL_SSBD;
    }
    if want != cur {
        // Only advertised bits are set, so this cannot #GP on spec-compliant
        // silicon; wrmsr_safe still guards a non-conforming emulator.
        unsafe {
            crate::msr::wrmsr_safe(IA32_SPEC_CTRL, want);
        }
    }
    spec_ctrl_read().unwrap_or(0)
}

/// Issue an Indirect Branch Prediction Barrier (IA32_PRED_CMD bit 0) on the
/// CURRENT CPU when supported — flushes the indirect-branch predictors so a
/// prior security domain cannot steer this one's speculation. A one-shot
/// command (not persistent MSR state); the scheduler issues it on a cross-
/// domain context switch. `#GP`-safe. Returns whether the barrier was issued.
pub fn issue_ibpb() -> bool {
    if !spec_ctrl_support().ibpb {
        return false;
    }
    unsafe { crate::msr::wrmsr_safe(IA32_PRED_CMD, 1) }
}

/// R10 FAIL-able hardening smoketest (BSP): if the CPU advertises any
/// persistent IA32_SPEC_CTRL branch-speculation control then, after
/// `enable_spec_ctrl()`, the read-back MSR MUST have every advertised bit
/// (IBRS/STIBP/SSBD) set; if the MSR is absent (TCG) or nothing is advertised
/// we honestly skip. A regression that leaves an advertised bit clear on a
/// capable CPU prints FAIL.
pub fn run_spec_ctrl_smoketest() {
    let sup = spec_ctrl_support();
    let readback = spec_ctrl_read();
    let (pass, active, present) = match readback {
        None => (true, 0u64, false), // MSR absent (emulator) — honest skip
        Some(v) => {
            let mut ok = true;
            if sup.ibrs && v & SPEC_CTRL_IBRS == 0 {
                ok = false;
            }
            if sup.stibp && v & SPEC_CTRL_STIBP == 0 {
                ok = false;
            }
            if sup.ssbd && v & SPEC_CTRL_SSBD == 0 {
                ok = false;
            }
            (ok, v, true)
        }
    };
    crate::serial_println!(
        "[cpu-harden] SPEC_CTRL: msr_present={} ibrs={} stibp={} ssbd={} ibpb={} readback={:#x} (real branch-spec MSR) -> {}",
        present,
        sup.ibrs,
        sup.stibp,
        sup.ssbd,
        sup.ibpb,
        active,
        if pass { "PASS" } else { "FAIL" }
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Transient-execution vulnerability posture (the Linux
// `/sys/devices/system/cpu/vulnerabilities` / Windows
// Get-SpeculationControlSettings equivalent).
//
// Given a CPU descriptor (vendor / family / model / IA32_ARCH_CAPABILITIES /
// the branch-spec MSR state), classify each known speculative-execution attack
// as Not-affected / Mitigated / Vulnerable. This is what a mature OS reports so
// the operator (and a security audit) knows the machine's EXACT posture — not a
// blanket "spectre=on". It caps the mitigation-honesty campaign: SMEP/SMAP/
// UMIP/heap-guard/SPEC_CTRL say what is ON; this says what the silicon is (and
// is not) vulnerable to and whether our defenses cover it.
//
// The assessment is a PURE FUNCTION of the descriptor, so the boot smoketest
// exercises it over synthetic silicon (a Zen4 and a Skylake) and can print FAIL
// if the gating logic regresses — then it reports the REAL CPU.
// ───────────────────────────────────────────────────────────────────────────

const IA32_ARCH_CAPABILITIES: u32 = 0x10A;
// IA32_ARCH_CAPABILITIES bits we consult.
const ARCH_CAP_RDCL_NO: u64 = 1 << 0; // not susceptible to Meltdown (RDCL)
const ARCH_CAP_IBRS_ALL: u64 = 1 << 1; // enhanced IBRS
const ARCH_CAP_SSB_NO: u64 = 1 << 4; // not susceptible to Spec Store Bypass
const ARCH_CAP_MDS_NO: u64 = 1 << 5; // not susceptible to MDS
const ARCH_CAP_TAA_NO: u64 = 1 << 8; // not susceptible to TAA

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VulnStatus {
    NotAffected,
    Mitigated,
    Vulnerable,
}

impl VulnStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            VulnStatus::NotAffected => "Not affected",
            VulnStatus::Mitigated => "Mitigated",
            VulnStatus::Vulnerable => "VULNERABLE",
        }
    }
}

/// A pure descriptor of one CPU's security-relevant identity so the assessment
/// logic can be unit-tested with synthetic silicon (no live CPUID needed).
#[derive(Clone, Copy, Debug)]
pub struct CpuSecurityDescriptor {
    pub vendor: crate::msr::CpuVendor,
    pub family: u32,
    pub model: u32,
    /// IA32_ARCH_CAPABILITIES (0x10A), or None if the CPU doesn't implement it
    /// (AMD parts generally don't — we then rely on vendor/family gating).
    pub arch_caps: Option<u64>,
    /// Which SPEC_CTRL bits the CPU advertises.
    pub spec_ctrl: SpecCtrlSupport,
    /// The SPEC_CTRL bits actually READ BACK set on this CPU (IBRS=1/STIBP=2/SSBD=4).
    pub spec_ctrl_active: u64,
    /// True if kernel↔user copies route through the validated uaccess bounds
    /// gate (the Spectre-v1 __user-pointer sanitization). Always true here.
    pub usercopy_hardened: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct VulnReport {
    pub meltdown: VulnStatus,
    pub spectre_v1: VulnStatus,
    pub spectre_v2: VulnStatus,
    pub spec_store_bypass: VulnStatus,
    pub l1tf: VulnStatus,
    pub mds: VulnStatus,
    pub taa: VulnStatus,
    pub srbds: VulnStatus,
    pub retbleed: VulnStatus,
}

/// Classify every known transient-execution attack for the given CPU. Pure
/// logic — no live hardware access — so it is host-/boot-testable with
/// synthetic descriptors. Verdicts are deliberately CONSERVATIVE (honest): a
/// CPU we cannot prove immune AND have not mitigated reads `Vulnerable`, never
/// an optimistic default.
pub fn assess_vulnerabilities(d: &CpuSecurityDescriptor) -> VulnReport {
    use crate::msr::CpuVendor;
    let caps = d.arch_caps.unwrap_or(0);
    let has = |bit: u64| d.arch_caps.is_some() && caps & bit != 0;
    let is_amd = d.vendor == CpuVendor::Amd;
    let is_intel = d.vendor == CpuVendor::Intel;
    let ibrs_active = d.spec_ctrl_active & (1 << 0) != 0 || has(ARCH_CAP_IBRS_ALL);
    let ssbd_active = d.spec_ctrl_active & (1 << 2) != 0;

    // Meltdown (CVE-2017-5754): AMD is architecturally immune; Intel is immune
    // only with RDCL_NO. We run a single per-task CR3 (no KPTI), so a vulnerable
    // Intel part is genuinely Vulnerable — reported honestly.
    let meltdown = if has(ARCH_CAP_RDCL_NO) || is_amd {
        VulnStatus::NotAffected
    } else if is_intel {
        VulnStatus::Vulnerable
    } else {
        VulnStatus::NotAffected
    };

    // Spectre v1 (CVE-2017-5753, bounds-check bypass): every speculating CPU is
    // architecturally affected; the mitigation is __user-pointer sanitization,
    // which our uaccess bounds gate provides.
    let spectre_v1 = if d.usercopy_hardened {
        VulnStatus::Mitigated
    } else {
        VulnStatus::Vulnerable
    };

    // Spectre v2 (CVE-2017-5715, branch-target injection): affected on all
    // speculating CPUs; mitigated by IBRS/eIBRS (or retpoline, which we don't
    // build). We program IBRS+STIBP in SPEC_CTRL.
    let spectre_v2 = if ibrs_active {
        VulnStatus::Mitigated
    } else {
        VulnStatus::Vulnerable
    };

    // Spec Store Bypass / Spectre v4 (CVE-2018-3639): immune with SSB_NO,
    // else mitigated by SSBD in SPEC_CTRL.
    let spec_store_bypass = if has(ARCH_CAP_SSB_NO) {
        VulnStatus::NotAffected
    } else if ssbd_active {
        VulnStatus::Mitigated
    } else {
        VulnStatus::Vulnerable
    };

    // L1TF (CVE-2018-3620): Intel-only (Core since Nehalem). AMD/other immune.
    // We do not do PTE inversion / conditional L1D flush, so a vulnerable Intel
    // part is reported honestly as Vulnerable.
    let l1tf = if is_amd || !is_intel {
        VulnStatus::NotAffected
    } else {
        VulnStatus::Vulnerable
    };

    // MDS (CVE-2018-12126/12127/12130, CVE-2019-11091): Intel-only; immune with
    // MDS_NO. AMD is not affected. We do not yet issue the VERW buffer clear on
    // Intel, so a vulnerable Intel part reads Vulnerable.
    let mds = if has(ARCH_CAP_MDS_NO) || is_amd || !is_intel {
        VulnStatus::NotAffected
    } else {
        VulnStatus::Vulnerable
    };

    // TAA (CVE-2019-11135): TSX Asynchronous Abort — requires Intel TSX. AMD has
    // no TSX; immune with TAA_NO.
    let taa = if has(ARCH_CAP_TAA_NO) || is_amd || !is_intel {
        VulnStatus::NotAffected
    } else {
        VulnStatus::Vulnerable
    };

    // SRBDS (CVE-2020-0543): specific Intel client parts. AMD not affected;
    // we conservatively report unmitigated Intel as Vulnerable.
    let srbds = if is_amd || !is_intel {
        VulnStatus::NotAffected
    } else {
        VulnStatus::Vulnerable
    };

    // Retbleed (CVE-2022-29900/29901): AMD Zen1/Zen2 (family 0x17) and Intel
    // Skylake-era. AMD Zen3/Zen4 (family 0x19) and later are NOT affected. On
    // affected parts, IBRS/eIBRS mitigates.
    let retbleed = if is_amd {
        if d.family >= 0x19 {
            VulnStatus::NotAffected // Zen3/Zen4+
        } else if d.family == 0x17 {
            if ibrs_active {
                VulnStatus::Mitigated
            } else {
                VulnStatus::Vulnerable
            }
        } else {
            VulnStatus::NotAffected // pre-Zen AMD not affected by Retbleed
        }
    } else if is_intel {
        if ibrs_active {
            VulnStatus::Mitigated
        } else {
            VulnStatus::Vulnerable
        }
    } else {
        VulnStatus::NotAffected
    };

    VulnReport {
        meltdown,
        spectre_v1,
        spectre_v2,
        spec_store_bypass,
        l1tf,
        mds,
        taa,
        srbds,
        retbleed,
    }
}

/// True if the CPU implements IA32_ARCH_CAPABILITIES — CPUID.(7,0):EDX[29].
fn arch_capabilities_supported() -> bool {
    core::arch::x86_64::__cpuid_count(7, 0).edx & (1 << 29) != 0
}

/// Build the live CPU's security descriptor from real CPUID + MSR reads.
pub fn current_cpu_security_descriptor() -> CpuSecurityDescriptor {
    let r1 = core::arch::x86_64::__cpuid(1);
    let base_model = (r1.eax >> 4) & 0xF;
    let ext_model = (r1.eax >> 16) & 0xF;
    let base_family = (r1.eax >> 8) & 0xF;
    // DisplayModel folds the extended model when base family is 0x6 or 0xF.
    let model = if base_family == 0xF || base_family == 0x6 {
        (ext_model << 4) | base_model
    } else {
        base_model
    };
    let arch_caps = if arch_capabilities_supported() {
        unsafe { crate::msr::rdmsr_safe(IA32_ARCH_CAPABILITIES) }
    } else {
        None
    };
    CpuSecurityDescriptor {
        vendor: crate::msr::cpu_vendor(),
        family: crate::msr::cpu_family(),
        model,
        arch_caps,
        spec_ctrl: spec_ctrl_support(),
        spec_ctrl_active: spec_ctrl_read().unwrap_or(0),
        // The uaccess bounds gate is compiled in unconditionally — every kernel
        // touch of user memory routes through it (Spectre-v1 sanitization).
        usercopy_hardened: true,
    }
}

/// `/proc/raeen/vulnerabilities` text — the live CPU's per-attack posture.
pub fn vulnerabilities_dump_text() -> alloc::string::String {
    use alloc::string::String;
    let d = current_cpu_security_descriptor();
    let r = assess_vulnerabilities(&d);
    let mut out = String::new();
    out.push_str("# RaeenOS CPU transient-execution vulnerability posture\n");
    out.push_str("# (Linux /sys .../vulnerabilities equivalent; Concept §Security)\n");
    out.push_str(&alloc::format!(
        "cpu                {:?} family={:#x} model={:#x} arch_caps={}\n",
        d.vendor,
        d.family,
        d.model,
        match d.arch_caps {
            Some(v) => alloc::format!("{:#x}", v),
            None => String::from("absent"),
        },
    ));
    let row = |out: &mut String, name: &str, s: VulnStatus| {
        out.push_str(&alloc::format!("{:<18} {}\n", name, s.as_str()));
    };
    row(&mut out, "meltdown", r.meltdown);
    row(&mut out, "spectre_v1", r.spectre_v1);
    row(&mut out, "spectre_v2", r.spectre_v2);
    row(&mut out, "spec_store_bypass", r.spec_store_bypass);
    row(&mut out, "l1tf", r.l1tf);
    row(&mut out, "mds", r.mds);
    row(&mut out, "taa", r.taa);
    row(&mut out, "srbds", r.srbds);
    row(&mut out, "retbleed", r.retbleed);
    out
}

/// R10 FAIL-able boot smoketest: run the assessment over two SYNTHETIC CPUs
/// with known-correct verdicts (a Zen4 that should be broadly Not-affected /
/// Mitigated, and a bare Skylake that should be Vulnerable to Meltdown +
/// Spectre-v2) so a regression in the gating logic prints FAIL — then report
/// the REAL CPU's posture. A test that can only pass is a false green; this one
/// has synthetic silicon that MUST come back Vulnerable.
pub fn run_vulnerabilities_smoketest() {
    use crate::msr::CpuVendor;
    // Synthetic Zen4 with IBRS/STIBP/SSBD active (readback 0x7).
    let zen4 = CpuSecurityDescriptor {
        vendor: CpuVendor::Amd,
        family: 0x19,
        model: 0x74,
        arch_caps: None,
        spec_ctrl: SpecCtrlSupport {
            ibrs: true,
            stibp: true,
            ssbd: true,
            ibpb: true,
        },
        spec_ctrl_active: 0x7,
        usercopy_hardened: true,
    };
    // Synthetic unmitigated Skylake: no arch-caps, no SPEC_CTRL bits set.
    let skylake = CpuSecurityDescriptor {
        vendor: CpuVendor::Intel,
        family: 0x6,
        model: 0x5E,
        arch_caps: None,
        spec_ctrl: SpecCtrlSupport::default(),
        spec_ctrl_active: 0,
        usercopy_hardened: true,
    };
    let z = assess_vulnerabilities(&zen4);
    let s = assess_vulnerabilities(&skylake);
    let zen4_ok = z.meltdown == VulnStatus::NotAffected
        && z.spectre_v2 == VulnStatus::Mitigated
        && z.spec_store_bypass == VulnStatus::Mitigated
        && z.mds == VulnStatus::NotAffected
        && z.retbleed == VulnStatus::NotAffected;
    let skylake_ok = s.meltdown == VulnStatus::Vulnerable
        && s.spectre_v2 == VulnStatus::Vulnerable
        && s.spec_store_bypass == VulnStatus::Vulnerable;
    let pass = zen4_ok && skylake_ok;

    // The real CPU's headline (the same data /proc/raeen/vulnerabilities dumps).
    let d = current_cpu_security_descriptor();
    let r = assess_vulnerabilities(&d);
    crate::serial_println!(
        "[cpu-harden] vulnerabilities: synth_zen4_ok={} synth_skylake_vulnerable={} | live {:?}: meltdown={} spectre_v2={} ssb={} mds={} retbleed={} -> {}",
        zen4_ok,
        skylake_ok,
        d.vendor,
        r.meltdown.as_str(),
        r.spectre_v2.as_str(),
        r.spec_store_bypass.as_str(),
        r.mds.as_str(),
        r.retbleed.as_str(),
        if pass { "PASS" } else { "FAIL" }
    );
}

pub fn init() {
    let info = build_snapshot();
    let v = match info.vendor {
        Vendor::Amd => "AMD",
        Vendor::Intel => "Intel",
        Vendor::Hypervisor => "Hypervisor",
        Vendor::Other => "Other",
    };
    let zen4_hint = is_amd_zen4(&info);
    crate::serial_println!(
        "[cpu] vendor={} family={:#x} model={:#x} stepping={} brand=\"{}\"{}",
        v,
        info.identity.family,
        info.identity.model,
        info.identity.stepping,
        info.brand,
        if zen4_hint {
            "  [Zen 4 detected — Athena profile]"
        } else {
            ""
        },
    );
    let f = &info.features;
    crate::serial_println!(
        "[cpu] core ISA: SSE2={} SSE4.2={} AVX={} AVX2={} AVX-512F={} AES={} SHA={} RDRAND={} RDSEED={}",
        f.sse2, f.sse42, f.avx, f.avx2, f.avx512f, f.aes, f.sha, f.rdrand, f.rdseed,
    );
    crate::serial_println!(
        "[cpu] kernel: x2APIC={} INVPCID={} SMEP={} SMAP={} UMIP={} CET-SS={} CET-IBT={} NX={} GBpages={} TSC-deadline={}",
        f.x2apic, f.invpcid, f.smep, f.smap, f.umip, f.cet_ss, f.cet_ibt, f.nx, f.gb_pages,
        cpuid_raw(1, 0).ecx & (1 << 24) != 0,
    );
    crate::serial_println!(
        "[cpu] memory tagging: LAM={} UAI={} (Concept §Security target)  virt: VMX={} SVM={}  hypervisor: kvm={} qemu={}",
        f.lam, f.uai, f.vmx, f.svm, f.hv_kvm, f.hv_qemu,
    );
    let t = &info.topology;
    let ct = match t.core_type {
        CoreType::IntelAtom => "Intel Atom (Efficiency)",
        CoreType::IntelCore => "Intel Core (Performance)",
        CoreType::AmdZen => "AMD Zen",
        CoreType::Unknown => "Unknown",
    };
    if info.features.hybrid {
        crate::serial_println!(
            "[cpu] topology: Hybrid core_type={} ranking={}",
            ct,
            t.efficiency_ranking
        );
    } else if info.vendor == Vendor::Amd {
        crate::serial_println!(
            "[cpu] topology: core_type={} compute_unit_id={} cores_per_cu={} node_id={}",
            ct,
            t.compute_unit_id,
            t.cores_per_compute_unit,
            t.node_id
        );
    } else {
        crate::serial_println!("[cpu] topology: core_type={}", ct);
    }
    for cache in &info.cache_topology {
        let type_str = match cache.cache_type {
            CacheType::Data => "Data",
            CacheType::Instruction => "Instruction",
            CacheType::Unified => "Unified",
            _ => "Null",
        };
        crate::serial_println!(
            "[cpu] L{} {} cache: {} KB, {} ways, {} sets, {} line, shared by {} core(s)",
            cache.level,
            type_str,
            cache.size_kb,
            cache.ways,
            cache.sets,
            cache.line_size,
            cache.sharing_count,
        );
    }
    *INFO.lock() = Some(info);
    crate::serial_println!("[ OK ] CPU feature detection complete");
}

pub fn run_boot_smoketest() {
    // Sanity: every feature we assume in our boot path must be present.
    let g = INFO.lock();
    let info = match g.as_ref() {
        Some(i) => i,
        None => return,
    };
    let mut missing = alloc::vec::Vec::new();
    if !info.features.fpu {
        missing.push("FPU");
    }
    if !info.features.tsc {
        missing.push("TSC");
    }
    if !info.features.msr {
        missing.push("MSR");
    }
    if !info.features.apic {
        missing.push("APIC");
    }
    if !info.features.sse2 {
        missing.push("SSE2");
    }
    if !info.features.cmpxchg16b {
        missing.push("CMPXCHG16B");
    }
    if !info.features.syscall {
        missing.push("SYSCALL/SYSRET");
    }
    if !info.features.nx {
        missing.push("NX");
    }
    if !info.features.lm_long_mode {
        missing.push("LM");
    }
    if info.topology.core_type == CoreType::Unknown {
        // Not strictly "missing" on all hardware, but we want to know if detection failed on modern chips.
        crate::serial_println!(
            "[cpu] [WARN] Core type unknown (topology detection failed or unsupported)"
        );
    }
    if info.cache_topology.is_empty() {
        missing.push("CACHE_TOPOLOGY");
    }
    if missing.is_empty() {
        crate::serial_println!("[cpu] smoketest OK: all required-baseline features present");
    } else {
        crate::serial_println!("[cpu] [WARN] missing required features: {:?}", missing);
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

/// AMD Family 0x19 + model 0x70..=0x7F = Zen 4 (Phoenix, Ryzen 7040 series
/// like the Beelink EliteMini Athena's Ryzen 5 7640HS).
fn is_amd_zen4(info: &CpuInfo) -> bool {
    info.vendor == Vendor::Amd && info.identity.family == 0x19 && (info.identity.model >> 4) == 0x7
}

/// Public test for whether we're on a Beelink Athena profile (Zen 4).
///
/// Init-order independent BY DESIGN: `hardware_profile::init()` (main.rs
/// Tier 8, early) calls this BEFORE `cpu_features::init()` populates `INFO`
/// (Tier 8, later) — the cached-INFO version returned `false` on every real
/// machine, so the unread-DMI fallback labeled Athena "profile = qemu"
/// (photographed twice). CPUID needs no initialization; query it directly.
pub fn is_athena_profile() -> bool {
    let leaf0 = cpuid_raw(0, 0);
    let mut vendor = [0u8; 12];
    vendor[0..4].copy_from_slice(&leaf0.ebx.to_le_bytes());
    vendor[4..8].copy_from_slice(&leaf0.edx.to_le_bytes());
    vendor[8..12].copy_from_slice(&leaf0.ecx.to_le_bytes());
    if &vendor != b"AuthenticAMD" {
        return false;
    }
    let id = read_identity(cpuid_raw(1, 0).eax, leaf0.eax);
    id.family == 0x19 && (id.model >> 4) == 0x7
}

/// Return a snapshot of detected CPU features (or defaults if uninitialized).
pub fn get_features() -> Features {
    let g = INFO.lock();
    g.as_ref().map(|i| i.features).unwrap_or_default()
}

// ── /proc/raeen/cpu ────────────────────────────────────────────────────

pub fn dump_text() -> String {
    let g = INFO.lock();
    let info = match g.as_ref() {
        Some(i) => i,
        None => return String::from("# cpu_features not initialized\n"),
    };
    let v = match info.vendor {
        Vendor::Amd => "AMD",
        Vendor::Intel => "Intel",
        Vendor::Hypervisor => "Hypervisor",
        Vendor::Other => "Other",
    };
    let mut out = String::new();
    out.push_str("# RaeenOS CPU feature detection\n");
    out.push_str(&alloc::format!("vendor: {} ({})\n", v, info.vendor_str));
    out.push_str(&alloc::format!("brand:  \"{}\"\n", info.brand));
    out.push_str(&alloc::format!(
        "family: 0x{:x}   model: 0x{:x}   stepping: {}\n",
        info.identity.family,
        info.identity.model,
        info.identity.stepping
    ));
    out.push_str(&alloc::format!(
        "max_basic_leaf: 0x{:x}   max_ext_leaf: 0x{:x}\n",
        info.identity.max_basic,
        info.identity.max_ext
    ));
    out.push_str(&alloc::format!("zen4_athena: {}\n", is_amd_zen4(info)));

    let t = &info.topology;
    let ct = match t.core_type {
        CoreType::IntelAtom => "Intel Atom (Efficiency)",
        CoreType::IntelCore => "Intel Core (Performance)",
        CoreType::AmdZen => "AMD Zen",
        CoreType::Unknown => "Unknown",
    };
    out.push_str(&alloc::format!("core_type: {}\n", ct));
    if info.features.hybrid {
        out.push_str(&alloc::format!(
            "hybrid_ranking: {}\n",
            t.efficiency_ranking
        ));
    }
    if info.vendor == Vendor::Amd {
        out.push_str(&alloc::format!(
            "amd_compute_unit_id: {}\n",
            t.compute_unit_id
        ));
        out.push_str(&alloc::format!(
            "amd_cores_per_cu:     {}\n",
            t.cores_per_compute_unit
        ));
        out.push_str(&alloc::format!("amd_node_id:          {}\n", t.node_id));
    }

    let f = &info.features;
    out.push_str("\n## features\n");
    let mut emit = |name: &str, on: bool| {
        out.push_str(&alloc::format!(
            "{:<16} {}\n",
            name,
            if on { "yes" } else { "no" }
        ));
    };
    emit("sse2", f.sse2);
    emit("sse4.2", f.sse42);
    emit("avx", f.avx);
    emit("avx2", f.avx2);
    emit("avx512f", f.avx512f);
    emit("avx512vbmi", f.avx512vbmi);
    emit("fma", f.fma);
    emit("aes", f.aes);
    emit("sha", f.sha);
    emit("rdrand", f.rdrand);
    emit("rdseed", f.rdseed);
    emit("x2apic", f.x2apic);
    emit("invpcid", f.invpcid);
    emit("fsgsbase", f.fsgsbase);
    emit("smep", f.smep);
    emit("smap", f.smap);
    emit("umip", f.umip);
    emit("pku", f.pku);
    emit("cet_ss", f.cet_ss);
    emit("cet_ibt", f.cet_ibt);
    emit("la57", f.la57);
    emit("nx", f.nx);
    emit("syscall", f.syscall);
    emit("rdtscp", f.rdtscp);
    emit("gb_pages", f.gb_pages);
    emit("vmx", f.vmx);
    emit("svm", f.svm);
    emit("hybrid", f.hybrid);
    emit("lam", f.lam);
    emit("uai", f.uai);
    emit("hv_kvm", f.hv_kvm);
    emit("hv_qemu", f.hv_qemu);

    out.push_str("\n## cache topology\n");
    for cache in &info.cache_topology {
        let type_str = match cache.cache_type {
            CacheType::Data => "Data",
            CacheType::Instruction => "Instruction",
            CacheType::Unified => "Unified",
            _ => "Null",
        };
        out.push_str(&alloc::format!(
            "L{} {:<12}: {:>6} KB, shared_by={}\n",
            cache.level,
            type_str,
            cache.size_kb,
            cache.sharing_count
        ));
    }
    out
}
