//! LinuxKPI host harness — validate the userspace driver shim's logic on the
//! dev host, no QEMU / no hardware.
//!
//! Two things this proves before Athena:
//!   1. The REAL `raeen_linuxkpi` primitives a Linux driver leans on — atomics,
//!      bit ops (ring/IRQ flags), MMIO register accessors, allocators (DMA
//!      descriptors) — behave correctly. Tested against the actual crate, not a
//!      copy (same code amdgpud/i915d link).
//!   2. A driver *bring-up sequence* (init handshake + ring-buffer setup + a
//!      doorbell/consume cycle) is replayed against a MOCK GPU register file
//!      using the real `readl`/`writel`, with a small hardware-reaction model.
//!      This is the framework for porting real amdgpu stages (GMC/IH/GFX) into a
//!      hardware-free regression test.
//!
//! What it does NOT prove: real AMD silicon behavior (timing, real register
//! semantics, firmware mailboxes) — that needs the 780M on bare metal. This
//! catches LOGIC/ORDERING bugs cheaply so the iron iteration is about hardware,
//! not software mistakes.

use raeen_amdgpu::{
    bringup::{self, DmaBuf, GpuOps},
    gc11,
};
use raeen_linuxkpi::{atomic, kalloc, mm, pci};
use std::collections::HashMap;
use std::sync::OnceLock;

/// True when `RAEEN_GPU_LOG` is set — surfaces the `[amdgpu]` bring-up stage
/// transcript (see `MockGpu::log`) instead of swallowing it, so `xtask gpu-test`
/// can capture the stage-by-stage GPU log on the host with no QEMU/iron. Read
/// once and cached so it stays cheap inside the per-line log callback.
fn gpu_log_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("RAEEN_GPU_LOG").is_ok())
}

// ── tiny PASS/FAIL harness (KAT-style serial lines, nonzero exit on failure) ──

struct Harness {
    pass: u32,
    fail: u32,
}
impl Harness {
    fn new() -> Self {
        Self { pass: 0, fail: 0 }
    }
    fn check(&mut self, name: &str, cond: bool) {
        if cond {
            self.pass += 1;
            println!("[linuxkpi-harness] {name}: PASS");
        } else {
            self.fail += 1;
            println!("[linuxkpi-harness] {name}: FAIL");
        }
    }
    fn finish(&self) -> ! {
        println!(
            "[linuxkpi-harness] summary: {} pass, {} fail -> {}",
            self.pass,
            self.fail,
            if self.fail == 0 { "PASS" } else { "FAIL" }
        );
        std::process::exit(if self.fail == 0 { 0 } else { 1 });
    }
}

// ── 1. Atomics (atomic_t / atomic64_t) — RMW correctness ──────────────────────

fn test_atomics(h: &mut Harness) {
    let mut v: i32 = 0;
    unsafe {
        atomic::atomic_set(&mut v, 5);
        h.check("atomic_set/read", atomic::atomic_read(&v) == 5);
        atomic::atomic_add(3, &mut v);
        h.check("atomic_add", atomic::atomic_read(&v) == 8);
        h.check("atomic_inc_return", atomic::atomic_inc_return(&mut v) == 9);
        h.check("atomic_dec_return", atomic::atomic_dec_return(&mut v) == 8);
        h.check(
            "atomic_cmpxchg hit",
            atomic::atomic_cmpxchg(&mut v, 8, 42) == 8,
        );
        h.check("atomic_cmpxchg applied", atomic::atomic_read(&v) == 42);
        h.check(
            "atomic_cmpxchg miss",
            atomic::atomic_cmpxchg(&mut v, 8, 99) == 42,
        );
        h.check("atomic_cmpxchg unchanged", atomic::atomic_read(&v) == 42);
        h.check("atomic_xchg", atomic::atomic_xchg(&mut v, 7) == 42);
        h.check("atomic_xchg applied", atomic::atomic_read(&v) == 7);
        // dec_and_test: 1 -> 0 returns true
        atomic::atomic_set(&mut v, 1);
        h.check(
            "atomic_dec_and_test true",
            atomic::atomic_dec_and_test(&mut v) == 1,
        );
        atomic::atomic_set(&mut v, 2);
        h.check(
            "atomic_dec_and_test false",
            atomic::atomic_dec_and_test(&mut v) == 0,
        );

        let mut v64: i64 = 0;
        atomic::atomic64_set(&mut v64, 1_000_000_000_000);
        atomic::atomic64_add(1, &mut v64);
        h.check(
            "atomic64_add",
            atomic::atomic64_read(&v64) == 1_000_000_000_001,
        );
        h.check(
            "atomic64_inc_return",
            atomic::atomic64_inc_return(&mut v64) == 1_000_000_000_002,
        );
    }
}

// ── 2. Bit ops (ring head/tail bitmaps, IRQ-pending masks) ────────────────────

fn test_bitops(h: &mut Harness) {
    let mut words: [u64; 2] = [0, 0]; // 128-bit array
    let addr = words.as_mut_ptr();
    unsafe {
        atomic::set_bit(3, addr);
        h.check("set_bit low", words[0] == 0b1000);
        atomic::set_bit(64, addr); // crosses into word[1]
        h.check("set_bit crosses word", words[1] == 0b1);
        h.check(
            "test_and_set already-set returns 1",
            atomic::test_and_set_bit(3, addr) == 1,
        );
        h.check(
            "test_and_set fresh returns 0",
            atomic::test_and_set_bit(5, addr) == 0,
        );
        h.check("test_and_set fresh now set", (words[0] >> 5) & 1 == 1);
        h.check(
            "test_and_clear set returns 1",
            atomic::test_and_clear_bit(5, addr) == 1,
        );
        h.check("test_and_clear now clear", (words[0] >> 5) & 1 == 0);
        atomic::clear_bit(3, addr);
        h.check("clear_bit", (words[0] >> 3) & 1 == 0);
    }
}

// ── 3. MMIO accessors (real readl/writel against a mock register file) ────────

fn test_mmio(h: &mut Harness) {
    let mut regs = vec![0u32; 16];
    let base = regs.as_mut_ptr();
    unsafe {
        pci::writel(0xDEAD_BEEF, base.add(2));
        h.check(
            "writel/readl u32",
            pci::readl(base.add(2) as *const u32) == 0xDEAD_BEEF,
        );

        let b = base as *mut u8;
        pci::writeb(0xAB, b.add(0));
        h.check("writeb/readb", pci::readb(b.add(0) as *const u8) == 0xAB);

        let w = base as *mut u16;
        pci::writew(0x1234, w.add(4));
        h.check("writew/readw", pci::readw(w.add(4) as *const u16) == 0x1234);
    }
}

// ── 4. Allocators (DMA descriptor / device-private structs) ───────────────────

fn test_alloc(h: &mut Harness) {
    unsafe {
        let p = mm::kzalloc(256, 0);
        h.check("kzalloc non-null", !p.is_null());
        if !p.is_null() {
            let zeroed = (0..256).all(|i| *p.add(i) == 0);
            h.check("kzalloc zeroes", zeroed);
        }
        // kcalloc overflow: n * size wraps usize -> must return null, not a
        // short buffer (the classic heap-overflow CVE pattern).
        let ov = kalloc::kcalloc(usize::MAX, 2, 0);
        h.check("kcalloc overflow -> null", ov.is_null());
        let ok = kalloc::kcalloc(16, 8, 0);
        h.check("kcalloc valid non-null", !ok.is_null());
        let arr_ov = kalloc::kmalloc_array(usize::MAX, 4, 0);
        h.check("kmalloc_array overflow -> null", arr_ov.is_null());

        // kmemdup copies; kstrdup NUL-terminates.
        let src = [1u8, 2, 3, 4, 5];
        let dup = kalloc::kmemdup(src.as_ptr(), src.len(), 0);
        let dup_ok = !dup.is_null() && (0..5).all(|i| *dup.add(i) == src[i]);
        h.check("kmemdup copies", dup_ok);
        let cstr = b"amdgpu\0";
        let sdup = kalloc::kstrdup(cstr.as_ptr(), 0);
        let sdup_ok = !sdup.is_null() && (0..7).all(|i| *sdup.add(i) == cstr[i]);
        h.check("kstrdup copies + NUL", sdup_ok);
    }
}

// ── 4b. Heap actually FREES (the Phase-1 bump allocator did not) ──────────────
//
// The decisive test for a running driver: alloc+free a chunk far more times
// than the 8 MiB heap could hold all at once. A bump allocator (kfree = no-op)
// returns null after ~heap_size/chunk iterations; a freeing heap never does.

fn test_heap_free(h: &mut Harness) {
    const CHUNK: usize = 64 * 1024; // 256 iters would fill 8 MiB if nothing freed
    let mut survived = true;
    for _ in 0..1000 {
        let p = mm::kmalloc(CHUNK, 0);
        if p.is_null() {
            survived = false;
            break;
        }
        unsafe {
            *p = 0xAB;
            *p.add(CHUNK - 1) = 0xCD;
        } // touch both ends
        mm::kfree(p);
    }
    h.check(
        "heap: 1000x alloc+free of 64KiB never exhausts (kfree reclaims)",
        survived,
    );

    // free + immediate reuse hands back usable memory.
    let a = mm::kmalloc(4096, 0);
    mm::kfree(a);
    let b = mm::kmalloc(4096, 0);
    h.check("heap: alloc after free succeeds", !b.is_null());
    mm::kfree(b);

    // krealloc grows and preserves the original bytes.
    let p = mm::kmalloc(8, 0);
    unsafe {
        for i in 0..8 {
            *p.add(i) = i as u8;
        }
    }
    let q = mm::krealloc(p, 64, false);
    let preserved = !q.is_null() && (0..8).all(|i| unsafe { *q.add(i) } == i as u8);
    h.check("heap: krealloc grows + preserves bytes", preserved);
    mm::kfree(q);

    // page allocator frees too (free_pages reconstructs size from order).
    let mut pages_ok = true;
    for _ in 0..1000 {
        let pg = mm::alloc_pages(2, false); // 4 pages = 16 KiB
        if pg.is_null() || !(pg as usize).is_multiple_of(4096) {
            pages_ok = false;
            break;
        }
        mm::free_pages(pg, 2);
    }
    h.check(
        "heap: 1000x alloc_pages(order=2)+free, page-aligned, no exhaust",
        pages_ok,
    );
}

// ── 4c. Locks: mutex_lock/spin_lock now route to the atomic acquire ───────────

fn test_locks(h: &mut Harness) {
    let mut w: u32 = 0;
    let p = &mut w as *mut u32;
    // mutex_lock must actually hold the word (was a non-atomic read-then-write).
    raeen_linuxkpi::mutex_lock(p);
    h.check(
        "lock: mutex_lock holds (trylock fails while held)",
        raeen_linuxkpi::sync::_raw_spin_trylock(p) == 0,
    );
    raeen_linuxkpi::mutex_unlock(p);
    h.check(
        "lock: mutex_unlock releases (trylock then succeeds)",
        raeen_linuxkpi::sync::_raw_spin_trylock(p) == 1,
    );
    raeen_linuxkpi::sync::_raw_spin_unlock(p);
    // spin_lock routes to the same atomic path.
    raeen_linuxkpi::spin_lock(p);
    h.check(
        "lock: spin_lock holds",
        raeen_linuxkpi::sync::_raw_spin_trylock(p) == 0,
    );
    raeen_linuxkpi::spin_unlock(p);
    h.check(
        "lock: spin_unlock releases",
        raeen_linuxkpi::sync::_raw_spin_trylock(p) == 1,
    );
    raeen_linuxkpi::sync::_raw_spin_unlock(p);
}

// ── 4d. printk %-specifier interpolation (was: format string printed verbatim) ─

fn test_printk_format(h: &mut Harness) {
    use raeen_linuxkpi::device::format_into;

    // Drive format_into with synthetic args (a fixed list consumed in order),
    // independent of the C vararg ABI — exercises the formatter itself.
    fn run(fmt: &[u8], args: &[u64], out: &mut [u8]) -> usize {
        let mut idx = 0usize;
        format_into(out, fmt, |_w64| {
            let v = args[idx];
            idx += 1;
            v
        })
    }

    let mut o = [0u8; 128];

    let n = run(
        b"val=%d u=%u hex=0x%x",
        &[(-42i32 as u32) as u64, 7, 0x1234],
        &mut o,
    );
    h.check(
        "printk: %d/%u/%x interpolate",
        &o[..n] == b"val=-42 u=7 hex=0x1234",
    );

    let cstr = b"gpu\0";
    let n = run(b"%s id=%03u", &[cstr.as_ptr() as u64, 5], &mut o);
    h.check("printk: %s + %03u zero-pad", &o[..n] == b"gpu id=005");

    let n = run(b"100%% @ %p", &[0xdead_beef], &mut o);
    h.check("printk: %% literal + %p", &o[..n] == b"100% @ 0xdeadbeef");

    let n = run(b"big=%llu", &[0x1_0000_0000], &mut o);
    h.check("printk: %llu 64-bit slot", &o[..n] == b"big=4294967296");

    let n = run(b"r=%y", &[], &mut o); // unknown specifier emitted literally
    h.check("printk: unknown %y not dropped", &o[..n] == b"r=%y");
}

// ── 5. Driver bring-up model (mock GPU register file + hardware reactions) ────
//
// A representative amdgpu-style block. Register offsets (byte):
//   CTRL    0x00  bit0 = GO
//   STATUS  0x04  bit0 = READY
//   RB_BASE_LO 0x08 / RB_BASE_HI 0x0C / RB_SIZE 0x10 / RB_RPTR 0x14 / RB_WPTR 0x18
//
// The driver uses the REAL readl/writel. `device_tick` models the silicon
// reacting (set READY after GO; advance RPTR toward WPTR as it "consumes" the
// ring). A bounded poll proves the bring-up logic terminates correctly — the
// exact class of ordering/handshake bug that otherwise only shows up on iron.

const CTRL: usize = 0x00;
const STATUS: usize = 0x04;
const RB_BASE_LO: usize = 0x08;
const RB_BASE_HI: usize = 0x0C;
const RB_SIZE: usize = 0x10;
const RB_RPTR: usize = 0x14;
const RB_WPTR: usize = 0x18;

#[inline]
unsafe fn reg(base: *mut u32, off: usize) -> *mut u32 {
    base.add(off / 4)
}

/// Model the GPU reacting to whatever the driver has written so far.
unsafe fn device_tick(base: *mut u32) {
    // Bring-up handshake: once the driver sets CTRL.GO, hardware asserts
    // STATUS.READY.
    if pci::readl(reg(base, CTRL) as *const u32) & 1 != 0 {
        pci::writel(1, reg(base, STATUS));
    }
    // Ring consume: advance RPTR one step toward WPTR each tick (models the GPU
    // draining the command ring).
    let rptr = pci::readl(reg(base, RB_RPTR) as *const u32);
    let wptr = pci::readl(reg(base, RB_WPTR) as *const u32);
    if rptr != wptr {
        pci::writel(rptr.wrapping_add(1), reg(base, RB_RPTR));
    }
}

fn test_driver_bringup(h: &mut Harness) {
    let mut regs = vec![0u32; 0x40];
    let base = regs.as_mut_ptr();
    unsafe {
        // Stage 1: init handshake — set GO, poll READY (bounded).
        pci::writel(1, reg(base, CTRL));
        let mut ready = false;
        for _ in 0..16 {
            device_tick(base);
            if pci::readl(reg(base, STATUS) as *const u32) & 1 != 0 {
                ready = true;
                break;
            }
        }
        h.check("bringup: GO -> READY handshake", ready);

        // Stage 2: ring-buffer setup — program base/size, zero pointers.
        let ring_dma: u64 = 0x1_2340_0000;
        pci::writel(ring_dma as u32, reg(base, RB_BASE_LO));
        pci::writel((ring_dma >> 32) as u32, reg(base, RB_BASE_HI));
        pci::writel(0x1000, reg(base, RB_SIZE)); // 4 KiB ring
        pci::writel(0, reg(base, RB_RPTR));
        pci::writel(0, reg(base, RB_WPTR));
        let lo = pci::readl(reg(base, RB_BASE_LO) as *const u32) as u64;
        let hi = pci::readl(reg(base, RB_BASE_HI) as *const u32) as u64;
        let readback = lo | (hi << 32);
        h.check("ring: 64-bit base programmed", readback == ring_dma);
        h.check(
            "ring: size programmed",
            pci::readl(reg(base, RB_SIZE) as *const u32) == 0x1000,
        );

        // Stage 3: submit — push WPTR to 8 commands, poll RPTR to converge
        // (models the GPU consuming the submitted ring).
        pci::writel(8, reg(base, RB_WPTR));
        let mut drained = false;
        for _ in 0..64 {
            device_tick(base);
            if pci::readl(reg(base, RB_RPTR) as *const u32)
                == pci::readl(reg(base, RB_WPTR) as *const u32)
            {
                drained = true;
                break;
            }
        }
        h.check("submit: RPTR converges to WPTR (ring drained)", drained);
    }
}

// ── 6. REAL amdgpu bring-up sequence (raeen_amdgpu::bringup) on a mock GPU ─────
//
// Test 5 is a *representative* model. This one drives amdgpud's ACTUAL stage
// sequence — the same `bringup::bringup` the live daemon runs — against a mock
// register file + DMA store, with a synthetic ATOMBIOS ROM so the real parser
// executes. If someone reorders a stage, renames a CP ring register, or breaks
// the GFX PM4 stream, THIS catches it on the host, before Athena.

// Mock register offsets for the gated handshakes, chosen clear of gc11's map
// (CP ~0x3040, GRBM ~0x8010, ucode ~0x5814) so they can't alias a real register.
const MOCK_SMU_MSG: u32 = 0x9000;
const MOCK_SMU_ARG: u32 = 0x9004;
const MOCK_SMU_RESP: u32 = 0x9008;
const MOCK_IH_BASE: u32 = 0x9100;
const MOCK_IH_BASE_HI: u32 = 0x9104;
const MOCK_IH_RPTR: u32 = 0x9108;
const MOCK_IH_WPTR: u32 = 0x910C;

/// Mock GPU: a sparse register file + a DMA buffer store + a hardware-reaction
/// model (CONFIG_MEMSIZE, the SMU mailbox, the IH ring, and a fence the GPU
/// posts on the CP doorbell).
struct MockGpu {
    present: bool,
    device_id: u16,
    present_fw: bool,
    rom: Option<Vec<u8>>,
    regs: HashMap<u32, u32>,
    /// id -> (dma_addr, dword contents) — lets us inspect what the driver DMA'd.
    dma: HashMap<u64, (u64, Vec<u32>)>,
    next_dma: u64,
    next_id: u64,
    scanout: Option<(u32, u32, u64)>,
    /// `(wptr_reg, fence_addr, fence_value)` per engine: writing a doorbell posts
    /// that engine's fence. A Vec so the GFX CP and SDMA fences can BOTH be
    /// modeled at once; empty during bring-up.
    complete_fence: Vec<(u32, u64, u32)>,
    /// IP-discovery blocks (None = not read, so gfx_regs falls back to gc11
    /// legacy); set to exercise the discovery-driven SOC15 offset path.
    gfx_discovery: Option<Vec<raeen_amdgpu::discovery::IpBlock>>,
    /// CP_ME_CNTL halt mask the mock honors; `Some` models a confirmed gfx11 RS64
    /// halt mask so the CP enable/halt path (`cp_gfx_enable`) actually writes.
    cp_me_cntl_halt_mask: Option<u32>,
}

impl MockGpu {
    fn new(present: bool, present_fw: bool, rom: Option<Vec<u8>>) -> Self {
        Self {
            present,
            device_id: bringup::RADEON_760M,
            present_fw,
            rom,
            regs: HashMap::new(),
            dma: HashMap::new(),
            next_dma: 0x1_0000_0000,
            next_id: 1,
            scanout: None,
            complete_fence: Vec::new(),
            gfx_discovery: None,
            cp_me_cntl_halt_mask: None,
        }
    }
}

impl GpuOps for MockGpu {
    fn pci_enable(&mut self, _b: u8, _d: u8, _f: u8) -> Option<u64> {
        if self.present {
            Some(0x42)
        } else {
            None
        }
    }
    fn config_read_dword(&mut self, _h: u64, off: u16) -> u32 {
        if off == 0 {
            (bringup::AMD_VENDOR as u32) | ((self.device_id as u32) << 16)
        } else {
            0
        }
    }
    fn map_register_bar(&mut self, _h: u64, _bar: u8) -> bool {
        true
    }
    fn reg_read(&mut self, off: u32) -> u32 {
        *self.regs.get(&off).unwrap_or(&0)
    }
    fn reg_write(&mut self, off: u32, val: u32) {
        self.regs.insert(off, val);
        // Model the PMFW answering OK when the SMU message id is written.
        if off == MOCK_SMU_MSG {
            self.regs.insert(MOCK_SMU_RESP, bringup::SMU_RESP_OK);
        }
        // Model each engine posting its fence to memory on a doorbell (WPTR
        // write). Clone the small Vec so we can iterate while mutating self.dma.
        let fences = self.complete_fence.clone();
        for (wptr_reg, fence_addr, fence_value) in fences {
            if off == wptr_reg {
                for (addr, contents) in self.dma.values_mut() {
                    if *addr == fence_addr {
                        if let Some(slot) = contents.get_mut(0) {
                            *slot = fence_value;
                        }
                    }
                }
            }
        }
    }
    fn read_vbios_rom(&mut self, _h: u64, _max: usize) -> Option<Vec<u8>> {
        self.rom.clone()
    }
    fn dma_alloc(&mut self, _h: u64, size: usize) -> Option<DmaBuf> {
        let id = self.next_id;
        self.next_id += 1;
        let dma_addr = self.next_dma;
        self.next_dma += size as u64;
        self.dma.insert(id, (dma_addr, vec![0u32; size / 4]));
        Some(DmaBuf { dma_addr, size, id })
    }
    fn dma_write(&mut self, buf: &DmaBuf, offset_dw: usize, data: &[u32]) {
        if let Some((_, contents)) = self.dma.get_mut(&buf.id) {
            for (i, w) in data.iter().enumerate() {
                if let Some(slot) = contents.get_mut(offset_dw + i) {
                    *slot = *w;
                }
            }
        }
    }
    fn dma_read(&mut self, buf: &DmaBuf, offset_dw: usize, out: &mut [u32]) {
        if let Some((_, contents)) = self.dma.get(&buf.id) {
            for (i, slot) in out.iter_mut().enumerate() {
                *slot = contents.get(offset_dw + i).copied().unwrap_or(0);
            }
        } else {
            out.iter_mut().for_each(|x| *x = 0);
        }
    }
    fn config_memsize_mb(&mut self) -> Option<u32> {
        Some(2048) // model Athena's BIOS UMA carve-out
    }
    fn smu_mailbox(&mut self) -> Option<bringup::SmuMailbox> {
        Some(bringup::SmuMailbox {
            msg_reg: MOCK_SMU_MSG,
            arg_reg: MOCK_SMU_ARG,
            resp_reg: MOCK_SMU_RESP,
        })
    }
    fn ih_ring(&mut self) -> Option<bringup::IhRing> {
        Some(bringup::IhRing {
            rb_base: MOCK_IH_BASE,
            rb_base_hi: MOCK_IH_BASE_HI,
            rb_rptr: MOCK_IH_RPTR,
            rb_wptr: MOCK_IH_WPTR,
        })
    }
    fn gfx_regs(&mut self) -> Option<bringup::GfxRegs> {
        self.gfx_discovery
            .as_ref()
            .and_then(|blocks| raeen_amdgpu::regs::gfx_regs(blocks))
    }
    fn cp_me_cntl_halt_mask(&mut self) -> Option<u32> {
        self.cp_me_cntl_halt_mask
    }
    fn sdma_regs(&mut self) -> Option<bringup::SdmaRegs> {
        self.gfx_discovery
            .as_ref()
            .and_then(|blocks| raeen_amdgpu::regs::sdma_regs(blocks))
    }
    fn request_firmware(&mut self, _name: &str) -> bool {
        self.present_fw
    }
    fn commit_scanout(&mut self, w: u32, h: u32, _pitch: u32, gpu_addr: u64) -> bool {
        self.scanout = Some((w, h, gpu_addr));
        true
    }
    fn log(&mut self, msg: &str) {
        // Off by default (CI stays quiet); `RAEEN_GPU_LOG` surfaces the full
        // amdgpu_device_init stage transcript for `xtask gpu-test`.
        if gpu_log_on() {
            println!("{msg}");
        }
    }
}

/// Minimal but structurally valid ATOMBIOS ROM (mirrors `atombios::tests::synth_rom`):
/// 0xAA55 sig, header ptr @0x48, "ATOM" sig + master cmd/data table offsets.
fn synth_atombios_rom() -> Vec<u8> {
    let len = 64 * 1024;
    let hp: usize = 0x80;
    let mut rom = vec![0u8; len];
    rom[0..2].copy_from_slice(&0xAA55u16.to_le_bytes());
    rom[0x48..0x4A].copy_from_slice(&(hp as u16).to_le_bytes());
    rom[hp..hp + 2].copy_from_slice(&36u16.to_le_bytes()); // structure_size
    rom[hp + 2] = 1; // format_revision
    rom[hp + 3] = 2; // content_revision
    rom[hp + 4..hp + 8].copy_from_slice(b"ATOM");
    rom[hp + 30..hp + 32].copy_from_slice(&0x0100u16.to_le_bytes()); // master cmd table
    rom[hp + 32..hp + 34].copy_from_slice(&0x0140u16.to_le_bytes()); // master data table
    rom
}

fn test_real_amdgpu_bringup(h: &mut Harness) {
    // Present GPU + firmware + valid VBIOS → every IP block should init.
    let mut gpu = MockGpu::new(true, true, Some(synth_atombios_rom()));
    let report = bringup::bringup(&mut gpu, &[(0, 1, 0), (3, 0, 0)]);
    h.check("amdgpu bringup: device present", report.device_present);
    h.check("amdgpu bringup: every IP block ok", report.all_ok());
    h.check("amdgpu bringup: scanout committed", gpu.scanout.is_some());

    // The CP GFX ring base register must read back as the gfx ring's DMA address.
    // Alloc order: IH (256 KiB) @0x1_0000_0000, then GFX @0x1_0004_0000.
    let base_lo = *gpu.regs.get(&gc11::MM_CP_RB0_BASE).unwrap_or(&0) as u64;
    let base_hi = *gpu.regs.get(&gc11::MM_CP_RB0_BASE_HI).unwrap_or(&0) as u64;
    let ring_base = base_lo | (base_hi << 32);
    h.check(
        "amdgpu bringup: CP_RB0_BASE = gfx ring dma_addr",
        ring_base == 0x1_0004_0000,
    );

    // The GFX ring DMA buffer must actually hold the PM4 stream the driver built:
    // the first dword is the IT_NOP PKT3 header (0xC000_1000).
    let mut ring_first = None;
    for (addr, contents) in gpu.dma.values() {
        if *addr == 0x1_0004_0000 {
            ring_first = contents.first().copied();
        }
    }
    h.check(
        "amdgpu bringup: gfx ring holds PM4 NOP header",
        ring_first == Some(0xC000_1000),
    );

    // The SDMA ring (3rd alloc @0x1_0005_0000) must hold the SDMA fill stream the
    // driver built: the first dword is the CONSTANT_FILL header (op 0x0B).
    let mut sdma_first = None;
    for (addr, contents) in gpu.dma.values() {
        if *addr == 0x1_0005_0000 {
            sdma_first = contents.first().copied();
        }
    }
    h.check(
        "amdgpu bringup: SDMA ring holds CONSTANT_FILL header",
        sdma_first == Some(raeen_amdgpu::sdma::SDMA_OP_CONST_FILL),
    );

    // CONFIG_MEMSIZE handshake: GMC sized VRAM from the mock's 2048 MiB.
    h.check(
        "amdgpu bringup: VRAM 2048 MiB from CONFIG_MEMSIZE",
        report.vram_mib == 2048,
    );
    // SMU mailbox handshake: init_smu sent a message; the PMFW model acked it.
    h.check(
        "amdgpu bringup: SMU mailbox acked",
        gpu.regs.get(&MOCK_SMU_RESP) == Some(&bringup::SMU_RESP_OK),
    );
    // IH ring programmed: base_hi holds the ring's high address (ring @0x1_0000_0000).
    h.check(
        "amdgpu bringup: IH ring base_hi programmed",
        gpu.regs.get(&MOCK_IH_BASE_HI) == Some(&1),
    );

    // No device at any probe BDF is the expected QEMU outcome — not a failure.
    let mut absent = MockGpu::new(false, false, None);
    let r2 = bringup::bringup(&mut absent, &[(0, 1, 0), (3, 0, 0)]);
    h.check(
        "amdgpu bringup: absent GPU -> not present",
        !r2.device_present,
    );
    h.check("amdgpu bringup: absent GPU -> not all_ok", !r2.all_ok());
}

/// The submit -> complete loop end to end on the mock GPU: write a PM4 stream,
/// ring the doorbell, wait for the fence. Proves both the wedged timeout and the
/// completion path through `bringup::submit_and_wait_fence`.
fn test_amdgpu_submit_fence(h: &mut Harness) {
    let mut gpu = MockGpu::new(true, true, None);
    let gfx = gpu.dma_alloc(0, 4096).unwrap();
    let fence = gpu.dma_alloc(0, 4096).unwrap();
    let wptr = gc11::MM_CP_RB0_WPTR;
    let fence_val = 0xCAFE_F00Du32;
    let stream = [0xC000_1000u32, 0]; // a PKT3 NOP

    // Wedged GPU: nothing posts the fence -> the wait must time out.
    let wedged =
        bringup::submit_and_wait_fence(&mut gpu, &gfx, wptr, &fence, fence_val, &stream, 4);
    h.check("amdgpu submit: wedged GPU -> timeout", !wedged);

    // Arm the fence: ringing the doorbell now posts fence_val to the fence buffer.
    gpu.complete_fence = vec![(wptr, fence.dma_addr, fence_val)];
    let done = bringup::submit_and_wait_fence(&mut gpu, &gfx, wptr, &fence, fence_val, &stream, 16);
    h.check("amdgpu submit: fence lands -> complete", done);
}

// ── 10. Workqueue + timer facade (deferred-exec, host-deterministic) ──────────
// Drives the real raeen_linuxkpi::workqueue pumps with controlled jiffies, so
// work/timer/delayed-work scheduling + cancellation is verified off-target.

use raeen_linuxkpi::workqueue::{self, TimerList, WorkStruct};
use std::sync::atomic::{AtomicU32, Ordering};

static WQ_WORK_RUNS: AtomicU32 = AtomicU32::new(0);
static WQ_TIMER_RUNS: AtomicU32 = AtomicU32::new(0);
static WQ_DWORK_RUNS: AtomicU32 = AtomicU32::new(0);

extern "C" fn wq_work_cb(_w: *mut WorkStruct) {
    WQ_WORK_RUNS.fetch_add(1, Ordering::SeqCst);
}
extern "C" fn wq_timer_cb(_t: *mut TimerList) {
    WQ_TIMER_RUNS.fetch_add(1, Ordering::SeqCst);
}
extern "C" fn wq_dwork_cb(_w: *mut WorkStruct) {
    WQ_DWORK_RUNS.fetch_add(1, Ordering::SeqCst);
}

fn test_workqueue(h: &mut Harness) {
    // ── work: schedule -> dedup -> pump runs it once ──
    let mut w = WorkStruct {
        data: 0,
        entry: [0, 0],
        func: Some(wq_work_cb),
    };
    let wp = &mut w as *mut WorkStruct;
    WQ_WORK_RUNS.store(0, Ordering::SeqCst);
    h.check("wq: schedule_work queues", workqueue::schedule_work(wp));
    h.check("wq: schedule_work dedups", !workqueue::schedule_work(wp));
    h.check(
        "wq: not run before pump",
        WQ_WORK_RUNS.load(Ordering::SeqCst) == 0,
    );
    let fired = workqueue::lkpi_run_work();
    h.check(
        "wq: pump runs work once",
        WQ_WORK_RUNS.load(Ordering::SeqCst) == 1 && fired >= 1,
    );

    // ── cancel before the pump -> never runs ──
    WQ_WORK_RUNS.store(0, Ordering::SeqCst);
    workqueue::schedule_work(wp);
    h.check(
        "wq: cancel_work_sync dequeues",
        workqueue::cancel_work_sync(wp),
    );
    workqueue::lkpi_run_work();
    h.check(
        "wq: cancelled work never ran",
        WQ_WORK_RUNS.load(Ordering::SeqCst) == 0,
    );

    // ── timer: absolute expiry, fires only at/after expiry, one-shot ──
    let mut t = TimerList {
        entry: [0, 0],
        expires: 0,
        function: None,
        flags: 0,
    };
    let tp = &mut t as *mut TimerList;
    workqueue::timer_setup(tp, wq_timer_cb, 0);
    WQ_TIMER_RUNS.store(0, Ordering::SeqCst);
    workqueue::mod_timer(tp, 100);
    workqueue::lkpi_run_timers(50);
    h.check(
        "timer: not fired before expiry",
        WQ_TIMER_RUNS.load(Ordering::SeqCst) == 0,
    );
    workqueue::lkpi_run_timers(100);
    h.check(
        "timer: fired at expiry",
        WQ_TIMER_RUNS.load(Ordering::SeqCst) == 1,
    );
    workqueue::lkpi_run_timers(200);
    h.check(
        "timer: one-shot (not re-fired)",
        WQ_TIMER_RUNS.load(Ordering::SeqCst) == 1,
    );

    // ── del_timer before expiry -> never fires ──
    WQ_TIMER_RUNS.store(0, Ordering::SeqCst);
    workqueue::mod_timer(tp, 300);
    h.check(
        "timer: del_timer_sync was active",
        workqueue::del_timer_sync(tp) == 1,
    );
    workqueue::lkpi_run_timers(300);
    h.check(
        "timer: deleted timer never fired",
        WQ_TIMER_RUNS.load(Ordering::SeqCst) == 0,
    );

    // ── delayed work: now+delay via the shim clock the pump advances ──
    let mut dw = WorkStruct {
        data: 0,
        entry: [0, 0],
        func: Some(wq_dwork_cb),
    };
    let dwp = &mut dw as *mut WorkStruct;
    WQ_DWORK_RUNS.store(0, Ordering::SeqCst);
    workqueue::lkpi_run_timers(1000); // advance shim NOW to 1000
    workqueue::schedule_delayed_work(dwp, 10); // -> expiry 1010
    workqueue::lkpi_run_timers(1005);
    h.check(
        "dwork: not fired before delay",
        WQ_DWORK_RUNS.load(Ordering::SeqCst) == 0,
    );
    workqueue::lkpi_run_timers(1010);
    h.check(
        "dwork: fired after delay",
        WQ_DWORK_RUNS.load(Ordering::SeqCst) == 1,
    );
}

// ── scatterlist (struct scatterlist / sg_table) ──────────────────────────────
// Exercises the REAL raeen_linuxkpi::scatterlist surface a GPU/DRM driver walks:
// init, set_buf round-trip (page base + in-page offset reconstruct the virtual
// address), the sg_next END-marker walk, sg_table alloc/free, and the identity
// dma_map_sg / dma_map_sgtable mapping. repr(C) structs hit the same offsets a
// real .ko would.

use raeen_linuxkpi::scatterlist::{self, Scatterlist, SgTable};

fn test_scatterlist(h: &mut Harness) {
    // ── sg_init_one: single entry round-trips the buffer ──
    let buf = vec![0u8; 256];
    let bp = buf.as_ptr() as *mut u8;
    let mut one = Scatterlist {
        page_link: 0,
        offset: 0,
        length: 0,
        dma_address: 0,
        dma_length: 0,
    };
    let op = &mut one as *mut Scatterlist;
    scatterlist::sg_init_one(op, bp, 256);
    h.check(
        "sg: init_one virt round-trips",
        scatterlist::sg_virt(op) == bp,
    );
    h.check("sg: init_one length", one.length == 256);
    h.check(
        "sg: init_one page+offset == buf",
        scatterlist::sg_page(op) as usize + one.offset as usize == bp as usize,
    );
    h.check(
        "sg: init_one is END (sg_next null)",
        scatterlist::sg_next(op).is_null(),
    );

    // ── sg_alloc_table(3): heap-backed array, last entry marked END ──
    let mut tbl = SgTable {
        sgl: core::ptr::null_mut(),
        nents: 0,
        orig_nents: 0,
    };
    let tp = &mut tbl as *mut SgTable;
    h.check(
        "sg: alloc_table ok",
        scatterlist::sg_alloc_table(tp, 3, 0) == 0,
    );
    h.check(
        "sg: alloc_table nents/orig",
        tbl.nents == 3 && tbl.orig_nents == 3 && !tbl.sgl.is_null(),
    );

    let bufs = [vec![0u8; 64], vec![0u8; 128], vec![0u8; 4096]];
    for (i, b) in bufs.iter().enumerate() {
        let sg = unsafe { tbl.sgl.add(i) };
        scatterlist::sg_set_buf(sg, b.as_ptr() as *mut u8, b.len() as u32);
    }
    // sg_set_buf must PRESERVE the END marker that sg_init_table set on entry 2.
    h.check(
        "sg: nents counts 3 (END preserved)",
        scatterlist::sg_nents(tbl.sgl) == 3,
    );

    // walk every entry via sg_next; verify lengths in order
    let mut sg = tbl.sgl;
    let mut walked = 0usize;
    let expect = [64u32, 128, 4096];
    let mut lengths_ok = true;
    while !sg.is_null() {
        if walked >= 3 || unsafe { (*sg).length } != expect[walked] {
            lengths_ok = false;
            break;
        }
        walked += 1;
        sg = scatterlist::sg_next(sg);
    }
    h.check(
        "sg: sg_next walk visits all 3 in order",
        walked == 3 && lengths_ok,
    );

    // ── dma_map_sg: identity dma_address/dma_length per entry ──
    let mapped = scatterlist::dma_map_sg(0xDEAD, tbl.sgl, 3, 1);
    h.check("sg: dma_map_sg maps 3", mapped == 3);
    let mut dma_ok = true;
    let mut sg = tbl.sgl;
    while !sg.is_null() {
        unsafe {
            if (*sg).dma_address != scatterlist::sg_phys(sg) || (*sg).dma_length != (*sg).length {
                dma_ok = false;
                break;
            }
        }
        sg = scatterlist::sg_next(sg);
    }
    h.check("sg: dma_map_sg fills addr/len identically", dma_ok);

    // ── dma_map_sgtable: maps orig_nents, records nents ──
    h.check(
        "sg: dma_map_sgtable ok",
        scatterlist::dma_map_sgtable(0xDEAD, tp, 1, 0) == 0 && tbl.nents == 3,
    );

    // ── sg_free_table: releases the heap array ──
    scatterlist::sg_free_table(tp);
    h.check(
        "sg: free_table clears sgl",
        tbl.sgl.is_null() && tbl.nents == 0,
    );
}

// ── refcount_t / kref ─────────────────────────────────────────────────────────
// Atomic refcount semantics over a driver-owned i32; kref_put fires the release
// callback exactly on the 1->0 transition (object teardown).

use raeen_linuxkpi::refcount;

static KREF_RELEASED: AtomicU32 = AtomicU32::new(0);
extern "C" fn kref_release_cb(_k: *mut i32) {
    KREF_RELEASED.fetch_add(1, Ordering::SeqCst);
}

fn test_refcount(h: &mut Harness) {
    let mut r: i32 = 0;
    let rp = &mut r as *mut i32;
    refcount::refcount_set(rp, 2);
    h.check("refcount: set/read", refcount::refcount_read(rp) == 2);
    refcount::refcount_inc(rp);
    h.check("refcount: inc -> 3", refcount::refcount_read(rp) == 3);
    h.check(
        "refcount: dec_and_test false above 1",
        !refcount::refcount_dec_and_test(rp),
    );
    h.check(
        "refcount: sub_and_test hits 0",
        refcount::refcount_sub_and_test(2, rp),
    );
    h.check(
        "refcount: inc_not_zero false at 0",
        !refcount::refcount_inc_not_zero(rp),
    );
    refcount::refcount_set(rp, 1);
    h.check(
        "refcount: inc_not_zero true above 0",
        refcount::refcount_inc_not_zero(rp) && refcount::refcount_read(rp) == 2,
    );

    // kref: release fires exactly on the last put
    let mut k: i32 = 0;
    let kp = &mut k as *mut i32;
    KREF_RELEASED.store(0, Ordering::SeqCst);
    refcount::kref_init(kp);
    refcount::kref_get(kp); // 2
    h.check(
        "kref: put with refs -> 0 ret, no release",
        refcount::kref_put(kp, Some(kref_release_cb)) == 0,
    );
    h.check(
        "kref: not released yet",
        KREF_RELEASED.load(Ordering::SeqCst) == 0,
    );
    h.check(
        "kref: last put -> 1 ret",
        refcount::kref_put(kp, Some(kref_release_cb)) == 1,
    );
    h.check(
        "kref: release fired once",
        KREF_RELEASED.load(Ordering::SeqCst) == 1,
    );
}

// ── ida / idr (ID allocators) ──────────────────────────────────────────────────
// Lowest-free allocation, range bounds, freeing reuses the slot, and idr's
// id->pointer round-trip. All over the daemon heap, no syscalls.

use raeen_linuxkpi::idr;

fn test_idr(h: &mut Harness) {
    // ── ida: dense lowest-free, free reuses ──
    let mut ida: usize = 0;
    let ip = &mut ida as *mut usize;
    let a0 = idr::ida_alloc(ip, 0);
    let a1 = idr::ida_alloc(ip, 0);
    let a2 = idr::ida_alloc(ip, 0);
    h.check("ida: dense 0,1,2", a0 == 0 && a1 == 1 && a2 == 2);
    idr::ida_free(ip, 1);
    h.check("ida: freed id reused", idr::ida_alloc(ip, 0) == 1);
    // range bound: min..=max, full -> ENOSPC
    let r0 = idr::ida_alloc_range(ip, 100, 101, 0);
    let r1 = idr::ida_alloc_range(ip, 100, 101, 0);
    let r2 = idr::ida_alloc_range(ip, 100, 101, 0);
    h.check(
        "ida: range 100,101 then full",
        r0 == 100 && r1 == 101 && r2 == -28,
    );
    // growth past initial 64-bit bitmap
    h.check(
        "ida: grows past initial cap",
        idr::ida_alloc_min(ip, 500, 0) == 500,
    );
    idr::ida_destroy(ip);
    h.check("ida: destroy resets handle", ida == 0);

    // ── idr: id -> pointer round-trip ──
    let mut idr_h: usize = 0;
    let dp = &mut idr_h as *mut usize;
    let obj_a = 0xA000usize as *mut u8;
    let obj_b = 0xB000usize as *mut u8;
    let id_a = idr::idr_alloc(dp, obj_a, 0, 0, 0);
    let id_b = idr::idr_alloc(dp, obj_b, 0, 0, 0);
    h.check("idr: dense ids", id_a == 0 && id_b == 1);
    h.check(
        "idr: find returns stored ptr",
        idr::idr_find(dp, id_a) == obj_a && idr::idr_find(dp, id_b) == obj_b,
    );
    h.check("idr: find missing -> null", idr::idr_find(dp, 99).is_null());
    let old = idr::idr_replace(dp, obj_b, id_a);
    h.check(
        "idr: replace returns old, stores new",
        old == obj_a && idr::idr_find(dp, id_a) == obj_b,
    );
    let removed = idr::idr_remove(dp, id_a);
    h.check(
        "idr: remove returns ptr, frees slot",
        removed == obj_b && idr::idr_find(dp, id_a).is_null(),
    );
    h.check("idr: not empty (id_b live)", idr::idr_is_empty(dp) == 0);
    idr::idr_remove(dp, id_b);
    h.check("idr: empty after all removed", idr::idr_is_empty(dp) == 1);
    // idr_init_base: first id honors the base
    idr::idr_destroy(dp);
    idr::idr_init_base(dp, 10);
    h.check(
        "idr: init_base honored",
        idr::idr_alloc(dp, obj_a, 0, 0, 0) == 10,
    );
    idr::idr_destroy(dp);
    h.check("idr: destroy resets handle", idr_h == 0);
}

// ── dma_pool (fixed-size coherent-DMA object allocator) ───────────────────────
// Backed by mm::alloc_pages (page-aligned, identity dma_addr). Verifies the
// create/alloc/zalloc/free/destroy contract: non-null object, handle written,
// page alignment, zeroing, and writability.

use raeen_linuxkpi::dma_pool;

fn test_dma_pool(h: &mut Harness) {
    let pool = dma_pool::dma_pool_create(core::ptr::null(), 0, 256, 64, 0);
    h.check("dma_pool: create ok", !pool.is_null());

    let mut handle: u64 = 0;
    let obj = dma_pool::dma_pool_alloc(pool, 0, &mut handle as *mut u64);
    h.check("dma_pool: alloc non-null", !obj.is_null());
    h.check("dma_pool: handle written (identity)", handle == obj as u64);
    h.check("dma_pool: object page-aligned", (obj as usize) % 4096 == 0);
    // writable across the requested object size
    unsafe {
        for i in 0..256usize {
            *obj.add(i) = 0xAB;
        }
    }
    h.check(
        "dma_pool: object writable",
        unsafe { *obj.add(255) } == 0xAB,
    );
    dma_pool::dma_pool_free(pool, obj, handle);

    // zalloc returns zeroed memory
    let mut h2: u64 = 0;
    let z = dma_pool::dma_pool_zalloc(pool, 0, &mut h2 as *mut u64);
    h.check("dma_pool: zalloc non-null", !z.is_null());
    let zeroed = (0..256usize).all(|i| unsafe { *z.add(i) == 0 });
    h.check("dma_pool: zalloc is zeroed", zeroed);
    dma_pool::dma_pool_free(pool, z, h2);

    // large object spans multiple pages and still allocates
    let big = dma_pool::dma_pool_create(core::ptr::null(), 0, 8192, 256, 0);
    let mut h3: u64 = 0;
    let bobj = dma_pool::dma_pool_alloc(big, 0, &mut h3 as *mut u64);
    h.check(
        "dma_pool: multi-page object",
        !bobj.is_null() && (bobj as usize) % 4096 == 0,
    );
    dma_pool::dma_pool_free(big, bobj, h3);
    dma_pool::dma_pool_destroy(big);

    dma_pool::dma_pool_destroy(pool);
    h.check(
        "dma_pool: zero-size create rejected",
        dma_pool::dma_pool_create(core::ptr::null(), 0, 0, 0, 0).is_null(),
    );
}

// ── kfifo (lockless SPSC ring) ────────────────────────────────────────────────
// Real ring logic: power-of-two sizing, free/used accounting, FIFO order, and
// wraparound across the buffer end. esize=4 (u32 elements).

use raeen_linuxkpi::kfifo::{self, Kfifo};

fn test_kfifo(h: &mut Harness) {
    let mut fifo = Kfifo {
        r#in: 0,
        out: 0,
        mask: 0,
        esize: 0,
        data: core::ptr::null_mut(),
    };
    let fp = &mut fifo as *mut Kfifo;
    // 4-element ring of u32 (size rounds up to pow2; 4 already is)
    h.check("kfifo: alloc ok", kfifo::__kfifo_alloc(fp, 4, 4, 0) == 0);
    h.check("kfifo: empty len 0", kfifo::__kfifo_len(fp) == 0);

    let src: [u32; 4] = [10, 20, 30, 40];
    let wrote = kfifo::__kfifo_in(fp, src.as_ptr() as *const u8, 4);
    h.check(
        "kfifo: in fills 4",
        wrote == 4 && kfifo::__kfifo_len(fp) == 4,
    );
    // full -> further in writes 0
    let extra: [u32; 1] = [99];
    h.check(
        "kfifo: in on full writes 0",
        kfifo::__kfifo_in(fp, extra.as_ptr() as *const u8, 1) == 0,
    );

    // peek does not consume
    let mut peek: [u32; 4] = [0; 4];
    let pk = kfifo::__kfifo_out_peek(fp, peek.as_mut_ptr() as *mut u8, 4);
    h.check(
        "kfifo: peek 4 in order",
        pk == 4 && peek == [10, 20, 30, 40] && kfifo::__kfifo_len(fp) == 4,
    );

    // out consumes 2 in FIFO order
    let mut got: [u32; 2] = [0; 2];
    let rd = kfifo::__kfifo_out(fp, got.as_mut_ptr() as *mut u8, 2);
    h.check(
        "kfifo: out 2 FIFO",
        rd == 2 && got == [10, 20] && kfifo::__kfifo_len(fp) == 2,
    );

    // refill 2 -> forces wraparound (out at 2, in at 4 -> writes land at idx 0,1)
    let more: [u32; 2] = [50, 60];
    h.check(
        "kfifo: refill wraps",
        kfifo::__kfifo_in(fp, more.as_ptr() as *const u8, 2) == 2,
    );
    // drain all 4: expect 30,40 (pre-wrap) then 50,60 (wrapped)
    let mut drain: [u32; 4] = [0; 4];
    let dr = kfifo::__kfifo_out(fp, drain.as_mut_ptr() as *mut u8, 4);
    h.check(
        "kfifo: drain across wrap in order",
        dr == 4 && drain == [30, 40, 50, 60],
    );
    h.check("kfifo: empty after drain", kfifo::__kfifo_len(fp) == 0);

    kfifo::__kfifo_free(fp);
    h.check(
        "kfifo: free clears data",
        fifo.data.is_null() && fifo.mask == 0,
    );

    // non-power-of-two size rounds up (5 -> 8)
    let mut f2 = Kfifo {
        r#in: 0,
        out: 0,
        mask: 0,
        esize: 0,
        data: core::ptr::null_mut(),
    };
    let f2p = &mut f2 as *mut Kfifo;
    kfifo::__kfifo_alloc(f2p, 5, 4, 0);
    h.check("kfifo: size rounds to pow2 (mask 7)", f2.mask == 7);
    kfifo::__kfifo_free(f2p);
}

// ── find_bit / bitmap ─────────────────────────────────────────────────────────
// Word-walk scan with partial-last-word masking. Bits packed LSB-first across
// two longs to exercise the cross-word boundary.

use raeen_linuxkpi::bitmap;

fn test_bitmap(h: &mut Harness) {
    // two-long bitmap (128 bits), all clear
    let mut map: [u64; 2] = [0, 0];
    let mp = map.as_mut_ptr();
    unsafe {
        h.check(
            "bitmap: empty find_first_bit == size",
            bitmap::_find_first_bit(mp, 128) == 128,
        );
        h.check(
            "bitmap: empty find_first_zero == 0",
            bitmap::_find_first_zero_bit(mp, 128) == 0,
        );

        // set bits 5, 64, 70
        bitmap::__bitmap_set(mp, 5, 1);
        bitmap::__bitmap_set(mp, 64, 1);
        bitmap::__bitmap_set(mp, 70, 1);
        h.check(
            "bitmap: find_first_bit == 5",
            bitmap::_find_first_bit(mp, 128) == 5,
        );
        h.check(
            "bitmap: find_next_bit after 5 crosses word to 64",
            bitmap::_find_next_bit(mp, 128, 6) == 64,
        );
        h.check(
            "bitmap: find_next_bit after 64 == 70",
            bitmap::_find_next_bit(mp, 128, 65) == 70,
        );
        h.check("bitmap: weight == 3", bitmap::__bitmap_weight(mp, 128) == 3);

        // a run via __bitmap_set, then clear part
        bitmap::__bitmap_set(mp, 100, 10); // 100..=109
        h.check(
            "bitmap: run set weight",
            bitmap::__bitmap_weight(mp, 128) == 13,
        );
        bitmap::__bitmap_clear(mp, 100, 5); // clear 100..=104
        h.check(
            "bitmap: after clear weight",
            bitmap::__bitmap_weight(mp, 128) == 8,
        );
        h.check(
            "bitmap: find_next_bit lands at 105",
            bitmap::_find_next_bit(mp, 128, 71) == 105,
        );

        // first zero in a partially full low word: bits 0..=4 set, 5 already set above? set 0..=4
        let mut m2: [u64; 1] = [0];
        let m2p = m2.as_mut_ptr();
        bitmap::__bitmap_set(m2p, 0, 5); // 0..=4
        h.check(
            "bitmap: find_first_zero skips set prefix -> 5",
            bitmap::_find_first_zero_bit(m2p, 64) == 5,
        );

        // full word: empty/full predicates
        let full: [u64; 1] = [u64::MAX];
        h.check(
            "bitmap: __bitmap_full true",
            bitmap::__bitmap_full(full.as_ptr(), 64) == 1,
        );
        h.check(
            "bitmap: __bitmap_full partial (32) true",
            bitmap::__bitmap_full(full.as_ptr(), 32) == 1,
        );
        let zero: [u64; 1] = [0];
        h.check(
            "bitmap: __bitmap_empty true",
            bitmap::__bitmap_empty(zero.as_ptr(), 64) == 1,
        );

        // or / complement / and over two longs
        let a: [u64; 2] = [0b1010, 0];
        let b: [u64; 2] = [0b0101, 0];
        let mut d: [u64; 2] = [0; 2];
        bitmap::__bitmap_or(d.as_mut_ptr(), a.as_ptr(), b.as_ptr(), 128);
        h.check("bitmap: or", d[0] == 0b1111);
        let any = bitmap::__bitmap_and(d.as_mut_ptr(), a.as_ptr(), b.as_ptr(), 128);
        h.check("bitmap: and disjoint -> 0", any == 0 && d[0] == 0);
        bitmap::__bitmap_complement(d.as_mut_ptr(), zero2().as_ptr(), 128);
        h.check(
            "bitmap: complement of 0 -> all ones",
            d[0] == u64::MAX && d[1] == u64::MAX,
        );

        // zalloc / free round-trip
        let bm = bitmap::bitmap_zalloc(200, 0);
        h.check(
            "bitmap: zalloc non-null + zeroed",
            !bm.is_null() && *bm == 0,
        );
        bitmap::__bitmap_set(bm, 3, 1);
        h.check(
            "bitmap: zalloc usable",
            bitmap::_find_first_bit(bm, 200) == 3,
        );
        bitmap::bitmap_free(bm);

        // to_arr32: split a long into two u32 halves
        let src: [u64; 1] = [0xDEADBEEF_CAFEF00D];
        let mut arr = [0u32; 2];
        bitmap::bitmap_to_arr32(arr.as_mut_ptr(), src.as_ptr(), 64);
        h.check(
            "bitmap: to_arr32 halves",
            arr[0] == 0xCAFEF00D && arr[1] == 0xDEADBEEF,
        );
    }
}

fn zero2() -> [u64; 2] {
    [0, 0]
}

// ── kstrto* (strict string -> integer) ────────────────────────────────────────
// Mirrors Linux: radix auto-detect, sign, trailing newline OK, junk -> -EINVAL,
// overflow -> -ERANGE. Error paths are the point (a lax parser misprograms hw).

use raeen_linuxkpi::kstrtox;

fn test_kstrtox(h: &mut Harness) {
    unsafe {
        let mut u: u64 = 0;
        h.check(
            "kstrto: ull decimal",
            kstrtox::kstrtoull(b"12345\0".as_ptr(), 10, &mut u) == 0 && u == 12345,
        );
        h.check(
            "kstrto: ull base0 hex",
            kstrtox::kstrtoull(b"0x1F\0".as_ptr(), 0, &mut u) == 0 && u == 31,
        );
        h.check(
            "kstrto: ull base0 octal",
            kstrtox::kstrtoull(b"0755\0".as_ptr(), 0, &mut u) == 0 && u == 0o755,
        );
        h.check(
            "kstrto: trailing newline ok",
            kstrtox::kstrtoull(b"42\n\0".as_ptr(), 10, &mut u) == 0 && u == 42,
        );
        h.check(
            "kstrto: junk -> EINVAL",
            kstrtox::kstrtoull(b"12x\0".as_ptr(), 10, &mut u) == -22,
        );
        h.check(
            "kstrto: empty -> EINVAL",
            kstrtox::kstrtoull(b"\0".as_ptr(), 10, &mut u) == -22,
        );

        let mut i: i64 = 0;
        h.check(
            "kstrto: ll negative",
            kstrtox::kstrtoll(b"-100\0".as_ptr(), 10, &mut i) == 0 && i == -100,
        );

        let mut i32v: i32 = 0;
        h.check(
            "kstrto: int ok",
            kstrtox::kstrtoint(b"-2000000000\0".as_ptr(), 10, &mut i32v) == 0
                && i32v == -2000000000,
        );
        h.check(
            "kstrto: int overflow -> ERANGE",
            kstrtox::kstrtoint(b"3000000000\0".as_ptr(), 10, &mut i32v) == -34,
        );

        let mut u32v: u32 = 0;
        h.check(
            "kstrto: uint ok",
            kstrtox::kstrtouint(b"4000000000\0".as_ptr(), 10, &mut u32v) == 0 && u32v == 4000000000,
        );
        h.check(
            "kstrto: uint overflow -> ERANGE",
            kstrtox::kstrtouint(b"5000000000\0".as_ptr(), 10, &mut u32v) == -34,
        );

        // from_user: counted buffer (no NUL)
        let raw = b"77 trailing garbage";
        h.check(
            "kstrto: from_user counted",
            kstrtox::kstrtouint_from_user(raw.as_ptr(), 2, 10, &mut u32v) == 0 && u32v == 77,
        );
    }
}

// ── s*printf cores ────────────────────────────────────────────────────────────
// Buffer formatting on the shared %-engine: content, size-bounding + NUL, and
// the two return conventions (snprintf = would-be length, scnprintf = bytes
// actually written). Synthetic varargs via a slice cursor (no C vararg ABI).

use raeen_linuxkpi::printf;

fn test_printf(h: &mut Harness) {
    // helper: build a "next" closure over a fixed arg list
    fn fmt_to(buf: &mut [u8], f: &[u8], args: &[u64]) -> i32 {
        let mut idx = 0usize;
        printf::vsnprintf_core(buf.as_mut_ptr(), buf.len(), f, |_w64| {
            let v = args.get(idx).copied().unwrap_or(0);
            idx += 1;
            v
        })
    }

    // exact fit: "x=42" (4 chars) into a roomy buffer
    let mut b = [0u8; 32];
    let n = fmt_to(&mut b, b"x=%d", &[42]);
    h.check("printf: returns would-be len 4", n == 4);
    h.check("printf: content + NUL", &b[..5] == b"x=42\0");

    // truncation: size 4 buffer holds 3 chars + NUL, returns would-be 4
    let mut small = [0xFFu8; 4];
    let n2 = printf::vsnprintf_core(small.as_mut_ptr(), 4, b"x=%d", {
        let mut i = 0;
        move |_w| {
            i += 1;
            if i == 1 {
                42
            } else {
                0
            }
        }
    });
    h.check(
        "printf: snprintf return is would-be (4) on truncation",
        n2 == 4,
    );
    h.check("printf: truncated to size-1 + NUL", &small == b"x=4\0");

    // scnprintf returns ACTUAL written (capped at size-1)
    let mut sc = [0u8; 4];
    let n3 = printf::scnprintf_core(sc.as_mut_ptr(), 4, b"x=%d", {
        let mut i = 0;
        move |_w| {
            i += 1;
            if i == 1 {
                42
            } else {
                0
            }
        }
    });
    h.check("printf: scnprintf returns actual-written 3", n3 == 3);

    // hex + string specifiers round-trip
    let mut hx = [0u8; 32];
    let hn = fmt_to(&mut hx, b"%x", &[0xDEAD]);
    h.check("printf: hex format", &hx[..hn as usize] == b"dead");
}

// ── sync/sem/completion extras + time extras (non-blocking paths) ─────────────
// trylock return polarity (Linux differs per call), mutex_is_locked, and the
// non-blocking completion queries — all syscall-free, so harness-safe.

use raeen_linuxkpi::{delay, sync};

fn test_sync_extras(h: &mut Harness) {
    // mutex_is_locked + down_trylock polarity (0 == success for down_trylock)
    let mut lock: u32 = 0;
    let lp = &mut lock as *mut u32;
    h.check(
        "sync: mutex_is_locked false initially",
        sync::mutex_is_locked(lp) == 0,
    );
    h.check(
        "sync: down_trylock acquires (ret 0)",
        sync::down_trylock(lp) == 0,
    );
    h.check(
        "sync: mutex_is_locked true after acquire",
        sync::mutex_is_locked(lp) == 1,
    );
    h.check(
        "sync: down_trylock fails when held (ret 1)",
        sync::down_trylock(lp) == 1,
    );
    sync::up(lp);
    h.check("sync: up releases", sync::mutex_is_locked(lp) == 0);

    // rwsem trylock returns 1 on success
    let mut rw: u32 = 0;
    let rp = &mut rw as *mut u32;
    h.check(
        "sync: down_read_trylock success (ret 1)",
        sync::down_read_trylock(rp) == 1,
    );
    h.check(
        "sync: down_read_trylock fail when held (ret 0)",
        sync::down_read_trylock(rp) == 0,
    );
    sync::up_read(rp);

    // completion: non-blocking try/done
    let mut c: u32 = 0;
    let cp = &mut c as *mut u32;
    sync::init_completion(cp);
    h.check(
        "sync: completion_done false initially",
        !sync::completion_done(cp),
    );
    h.check(
        "sync: try_wait empty -> false",
        !sync::try_wait_for_completion(cp),
    );
    sync::complete(cp);
    h.check(
        "sync: completion_done after complete",
        sync::completion_done(cp),
    );
    h.check(
        "sync: try_wait consumes one",
        sync::try_wait_for_completion(cp),
    );
    h.check(
        "sync: try_wait empty again -> false",
        !sync::try_wait_for_completion(cp),
    );

    // time conversions
    h.check("time: jiffies_to_usecs", delay::jiffies_to_usecs(5) == 5000);
    h.check(
        "time: usecs_to_jiffies rounds up",
        delay::usecs_to_jiffies(1500) == 2,
    );
    let mut ts = delay::Timespec64 {
        tv_sec: -1,
        tv_nsec: -1,
    };
    delay::ktime_get_ts64(&mut ts as *mut delay::Timespec64);
    h.check(
        "time: ktime_get_ts64 nsec in range",
        ts.tv_nsec >= 0 && ts.tv_nsec < 1_000_000_000,
    );
}

// ── modern slab ABI ───────────────────────────────────────────────────────────
// kmalloc_trace/__kmalloc/kmalloc_large/kvmalloc_node all route to the daemon
// heap; kmalloc_caches is the indexable zero array the kmalloc() inline reads.

fn test_slab(h: &mut Harness) {
    let p = kalloc::__kmalloc(128, 0);
    h.check("slab: __kmalloc non-null", !p.is_null());
    unsafe {
        for i in 0..128 {
            *p.add(i) = 0x5A;
        }
    }
    h.check("slab: __kmalloc writable", unsafe { *p.add(127) } == 0x5A);
    mm::kfree(p);

    let t = kalloc::kmalloc_trace(core::ptr::null_mut(), 0, 64);
    h.check(
        "slab: kmalloc_trace allocs by size (cache ignored)",
        !t.is_null(),
    );
    mm::kfree(t);

    let big = kalloc::kmalloc_large(9000, 0);
    h.check("slab: kmalloc_large non-null", !big.is_null());
    mm::kfree(big);

    let kv = kalloc::kvmalloc_node(256, 0, 0);
    h.check("slab: kvmalloc_node non-null", !kv.is_null());
    mm::kfree(kv);

    h.check(
        "slab: kmalloc_caches[3][12] indexable + null",
        kalloc::kmalloc_caches[3][12] == 0,
    );

    let a = kalloc::__kmalloc(8, 0);
    unsafe { *a = 0xEE };
    let a2 = kalloc::krealloc_array(a, 4, 8, 0);
    h.check(
        "slab: krealloc_array preserves prefix",
        !a2.is_null() && unsafe { *a2 } == 0xEE,
    );
    mm::kfree(a2);
}

// ── PCIe extended-capability walk ─────────────────────────────────────────────
// The pure walk logic over a mock config space: chained headers, found/not-found,
// and loop termination on a malformed (self/zero next) chain.

fn test_pci_ext_cap(h: &mut Harness) {
    // mock ext config: 0x100 -> cap 0x0001 next 0x140; 0x140 -> cap 0x000B next 0
    let read = |off: u16| -> u32 {
        match off {
            0x100 => 0x0001 | (0x1 << 16) | (0x140 << 20),
            0x140 => 0x000B | (0x1 << 16) | (0 << 20),
            _ => 0,
        }
    };
    use raeen_linuxkpi::pci_ext::walk_ext_cap;
    h.check(
        "pci: ext cap found at head",
        walk_ext_cap(read, 0x0001) == 0x100,
    );
    h.check(
        "pci: ext cap found after chain",
        walk_ext_cap(read, 0x000B) == 0x140,
    );
    h.check("pci: ext cap absent -> 0", walk_ext_cap(read, 0x00FF) == 0);
    // all-ones header (no device) terminates immediately
    h.check(
        "pci: no-device header -> 0",
        walk_ext_cap(|_| 0xFFFF_FFFF, 0x0001) == 0,
    );
    // self-referential next must not loop forever (guard)
    h.check(
        "pci: self-loop guarded",
        walk_ext_cap(|_| 0x0099 | (0x100 << 20), 0x0001) == 0,
    );
}

// ── dma_fence (dma-buf async completion) ──────────────────────────────────────
// Real fence semantics against the BTF-verified struct layout: context counter,
// init, callback queue fires exactly once on signal, double-signal/ENOENT
// guards, remove_callback, wait fast-path, and the always-signaled stub.

use raeen_linuxkpi::dma_fence::{self, DmaFence, DmaFenceCb, ListHead};

static FENCE_CB_RUNS: AtomicU32 = AtomicU32::new(0);
extern "C" fn fence_cb(_f: *mut DmaFence, _cb: *mut DmaFenceCb) {
    FENCE_CB_RUNS.fetch_add(1, Ordering::SeqCst);
}

fn blank_fence() -> DmaFence {
    DmaFence {
        lock: std::ptr::null_mut(),
        ops: std::ptr::null(),
        cb_list: ListHead {
            next: std::ptr::null_mut(),
            prev: std::ptr::null_mut(),
        },
        context: 0,
        seqno: 0,
        flags: 0,
        refcount: 0,
        error: 0,
    }
}
fn blank_cb() -> DmaFenceCb {
    DmaFenceCb {
        node: ListHead {
            next: std::ptr::null_mut(),
            prev: std::ptr::null_mut(),
        },
        func: None,
    }
}

fn test_dma_fence(h: &mut Harness) {
    unsafe {
        let c1 = dma_fence::dma_fence_context_alloc(1);
        let c2 = dma_fence::dma_fence_context_alloc(2);
        let c3 = dma_fence::dma_fence_context_alloc(1);
        h.check(
            "dma_fence: context_alloc monotonic",
            c2 == c1 + 1 && c3 == c1 + 3,
        );

        let mut lk: u32 = 0;
        let ops = &dma_fence::raeen_dma_fence_stub_ops as *const dma_fence::DmaFenceOps;
        let mut f = blank_fence();
        let fp = &mut f as *mut DmaFence;
        dma_fence::dma_fence_init(fp, ops, &mut lk as *mut u32 as *mut u8, 100, 7);
        h.check(
            "dma_fence: init sets context/seqno/refcount",
            f.context == 100 && f.seqno == 7 && f.refcount == 1,
        );

        FENCE_CB_RUNS.store(0, Ordering::SeqCst);
        let mut cb = blank_cb();
        let cbp = &mut cb as *mut DmaFenceCb;
        h.check(
            "dma_fence: add_callback queues (0)",
            dma_fence::dma_fence_add_callback(fp, cbp, Some(fence_cb)) == 0,
        );
        h.check(
            "dma_fence: cb not fired pre-signal",
            FENCE_CB_RUNS.load(Ordering::SeqCst) == 0,
        );
        h.check(
            "dma_fence: signal -> 0",
            dma_fence::dma_fence_signal(fp) == 0,
        );
        h.check(
            "dma_fence: cb fired exactly once",
            FENCE_CB_RUNS.load(Ordering::SeqCst) == 1,
        );
        h.check(
            "dma_fence: double-signal -> EINVAL",
            dma_fence::dma_fence_signal(fp) == -22,
        );
        h.check(
            "dma_fence: add_callback after signal -> ENOENT",
            dma_fence::dma_fence_add_callback(fp, cbp, Some(fence_cb)) == -2,
        );
        h.check(
            "dma_fence: wait_timeout signaled fast-path > 0",
            dma_fence::dma_fence_wait_timeout(fp, false, 50) > 0,
        );

        // remove_callback before signal prevents the callback from firing
        let mut lk2: u32 = 0;
        let mut f2 = blank_fence();
        let f2p = &mut f2 as *mut DmaFence;
        dma_fence::dma_fence_init(f2p, ops, &mut lk2 as *mut u32 as *mut u8, 1, 1);
        FENCE_CB_RUNS.store(0, Ordering::SeqCst);
        let mut cb2 = blank_cb();
        let cb2p = &mut cb2 as *mut DmaFenceCb;
        dma_fence::dma_fence_add_callback(f2p, cb2p, Some(fence_cb));
        h.check(
            "dma_fence: remove_callback returns true",
            dma_fence::dma_fence_remove_callback(f2p, cb2p),
        );
        dma_fence::dma_fence_signal(f2p);
        h.check(
            "dma_fence: removed cb never fired",
            FENCE_CB_RUNS.load(Ordering::SeqCst) == 0,
        );

        let stub = dma_fence::dma_fence_get_stub();
        h.check(
            "dma_fence: get_stub is signaled",
            dma_fence::dma_fence_wait_timeout(stub, false, 10) > 0,
        );
    }
}

fn test_dma_fence_chain(h: &mut Harness) {
    unsafe {
        let ops = &dma_fence::raeen_dma_fence_stub_ops as *const dma_fence::DmaFenceOps;
        // inner work fences
        let mut lk1: u32 = 0;
        let mut lk2: u32 = 0;
        let mut inner1 = blank_fence();
        let mut inner2 = blank_fence();
        let i1 = &mut inner1 as *mut DmaFence;
        let i2 = &mut inner2 as *mut DmaFence;
        dma_fence::dma_fence_init(i1, ops, &mut lk1 as *mut u32 as *mut u8, 9, 1);
        dma_fence::dma_fence_init(i2, ops, &mut lk2 as *mut u32 as *mut u8, 9, 2);

        // chain: link2 -> link1(prev), each wrapping an inner fence
        let mut chain1: dma_fence::DmaFenceChain = std::mem::zeroed();
        let mut chain2: dma_fence::DmaFenceChain = std::mem::zeroed();
        let c1 = &mut chain1 as *mut dma_fence::DmaFenceChain;
        let c2 = &mut chain2 as *mut dma_fence::DmaFenceChain;
        dma_fence::dma_fence_chain_init(c1, std::ptr::null_mut(), i1, 1);
        let c1base = core::ptr::addr_of_mut!((*c1).base);
        dma_fence::dma_fence_chain_init(c2, c1base, i2, 2);
        let c2base = core::ptr::addr_of_mut!((*c2).base);

        // ops identity check works (drivers detect chains this way)
        h.check(
            "dma_fence_chain: ops identity",
            (*c2base).ops == core::ptr::addr_of!(dma_fence::dma_fence_chain_ops),
        );
        // walk from c2: prev (c1) wraps an unsignaled inner -> returned
        h.check(
            "dma_fence_chain: walk returns pending prev",
            dma_fence::dma_fence_chain_walk(c2base) == c1base,
        );
        // signal inner1 -> walk now finds nothing pending behind c2
        dma_fence::dma_fence_signal(i1);
        h.check(
            "dma_fence_chain: walk null once prev signaled",
            dma_fence::dma_fence_chain_walk(c2base).is_null(),
        );
    }
}

fn test_dma_fence_array(h: &mut Harness) {
    unsafe {
        let ops = &dma_fence::raeen_dma_fence_stub_ops as *const dma_fence::DmaFenceOps;
        let mut lk1: u32 = 0;
        let mut lk2: u32 = 0;
        let mut c1 = blank_fence();
        let mut c2 = blank_fence();
        let c1p = &mut c1 as *mut DmaFence;
        let c2p = &mut c2 as *mut DmaFence;
        dma_fence::dma_fence_init(c1p, ops, &mut lk1 as *mut u32 as *mut u8, 1, 1);
        dma_fence::dma_fence_init(c2p, ops, &mut lk2 as *mut u32 as *mut u8, 1, 2);

        let mut children: [*mut DmaFence; 2] = [c1p, c2p];
        let arr = dma_fence::dma_fence_array_create(2, children.as_mut_ptr(), 500, 1, false);
        h.check("dma_fence_array: create non-null", !arr.is_null());
        let base = core::ptr::addr_of_mut!((*arr).base);
        // timeout 0 -> non-blocking "is it signaled?" probe (no syscall)
        h.check(
            "dma_fence_array: not signaled with 2 pending",
            dma_fence::dma_fence_wait_timeout(base, false, 0) == 0,
        );

        dma_fence::dma_fence_signal(c1p);
        h.check(
            "dma_fence_array: still pending after 1 child",
            dma_fence::dma_fence_wait_timeout(base, false, 0) == 0,
        );

        dma_fence::dma_fence_signal(c2p);
        h.check(
            "dma_fence_array: signals when all children done",
            dma_fence::dma_fence_wait_timeout(base, false, 0) > 0,
        );

        // release the array via its base kref (frees the kzalloc'd struct)
        let kref = (base as usize + core::mem::offset_of!(DmaFence, refcount)) as *mut u8;
        dma_fence::dma_fence_release(kref);

        // signal_on_any: first child signaling is enough
        let mut lk3: u32 = 0;
        let mut lk4: u32 = 0;
        let mut d1 = blank_fence();
        let mut d2 = blank_fence();
        let d1p = &mut d1 as *mut DmaFence;
        let d2p = &mut d2 as *mut DmaFence;
        dma_fence::dma_fence_init(d1p, ops, &mut lk3 as *mut u32 as *mut u8, 2, 1);
        dma_fence::dma_fence_init(d2p, ops, &mut lk4 as *mut u32 as *mut u8, 2, 2);
        let mut any: [*mut DmaFence; 2] = [d1p, d2p];
        let arr2 = dma_fence::dma_fence_array_create(2, any.as_mut_ptr(), 600, 1, true);
        let base2 = core::ptr::addr_of_mut!((*arr2).base);
        dma_fence::dma_fence_signal(d1p);
        h.check(
            "dma_fence_array: signal_on_any fires on first",
            dma_fence::dma_fence_wait_timeout(base2, false, 0) > 0,
        );
        dma_fence::dma_fence_release(
            (base2 as usize + core::mem::offset_of!(DmaFence, refcount)) as *mut u8,
        );
    }
}

// ── dma_resv (reservation object: fences on a buffer) ─────────────────────────
// reserve/add, usage-filtered iteration (READ yields KERNEL/WRITE/READ; WRITE
// yields only KERNEL/WRITE), test_signaled, get_fences, get_singleton.

use raeen_linuxkpi::dma_resv::{self, DmaResv, DmaResvIter};

const USAGE_WRITE: u32 = 1;
const USAGE_READ: u32 = 2;

fn test_dma_resv(h: &mut Harness) {
    unsafe {
        let ops = &dma_fence::raeen_dma_fence_stub_ops as *const dma_fence::DmaFenceOps;
        let mut obj: DmaResv = std::mem::zeroed();
        let op = &mut obj as *mut DmaResv;
        dma_resv::dma_resv_init(op);

        h.check(
            "dma_resv: reserve_fences ok",
            dma_resv::dma_resv_reserve_fences(op, 2) == 0,
        );

        let mut lk1: u32 = 0;
        let mut lk2: u32 = 0;
        let mut w = blank_fence();
        let mut r = blank_fence();
        let wp = &mut w as *mut DmaFence;
        let rp = &mut r as *mut DmaFence;
        dma_fence::dma_fence_init(wp, ops, &mut lk1 as *mut u32 as *mut u8, 10, 1); // context 10 = writer
        dma_fence::dma_fence_init(rp, ops, &mut lk2 as *mut u32 as *mut u8, 20, 1); // context 20 = reader
        dma_resv::dma_resv_add_fence(op, wp, USAGE_WRITE);
        dma_resv::dma_resv_add_fence(op, rp, USAGE_READ);

        // iterate for READ -> both; for WRITE -> only the writer
        let count_for = |usage: u32| -> u32 {
            let mut it: DmaResvIter = std::mem::zeroed();
            it.obj = op;
            it.usage = usage;
            let mut n = 0u32;
            let mut f = dma_resv::dma_resv_iter_first(&mut it as *mut DmaResvIter);
            while !f.is_null() {
                n += 1;
                f = dma_resv::dma_resv_iter_next(&mut it as *mut DmaResvIter);
            }
            n
        };
        h.check(
            "dma_resv: iterate READ yields 2",
            count_for(USAGE_READ) == 2,
        );
        h.check(
            "dma_resv: iterate WRITE yields 1",
            count_for(USAGE_WRITE) == 1,
        );

        h.check(
            "dma_resv: not signaled initially",
            !dma_resv::dma_resv_test_signaled(op, USAGE_READ),
        );

        // get_fences for READ -> 2
        let mut num: u32 = 0;
        let mut arr: *mut *mut DmaFence = std::ptr::null_mut();
        h.check(
            "dma_resv: get_fences returns 2",
            dma_resv::dma_resv_get_fences(op, USAGE_READ, &mut num, &mut arr) == 0 && num == 2,
        );
        if !arr.is_null() {
            raeen_linuxkpi::mm::kfree(arr as *mut u8);
        }

        // signal both -> test_signaled true, wait fast-path > 0
        dma_fence::dma_fence_signal(wp);
        h.check(
            "dma_resv: still pending after writer",
            !dma_resv::dma_resv_test_signaled(op, USAGE_READ),
        );
        dma_fence::dma_fence_signal(rp);
        h.check(
            "dma_resv: signaled after both",
            dma_resv::dma_resv_test_signaled(op, USAGE_READ),
        );
        h.check(
            "dma_resv: wait_timeout fast-path > 0",
            dma_resv::dma_resv_wait_timeout(op, USAGE_READ, false, 0) > 0,
        );

        // get_singleton for READ with 2 fences -> a (non-null) fence-array
        let mut single: *mut DmaFence = std::ptr::null_mut();
        h.check(
            "dma_resv: get_singleton builds a fence",
            dma_resv::dma_resv_get_singleton(op, USAGE_READ, &mut single) == 0 && !single.is_null(),
        );

        dma_resv::dma_resv_fini(op);
    }
}

// ── dma_buf (importer dispatch over a mock exporter) ──────────────────────────
// Drives the real dma_buf_* dispatch path against a fake exporter vtable:
// attach/map/unmap/pin/unpin reach the exporter's ops; move_notify reaches the
// importer; dma_buf_get reports the documented host-brokering limitation.

use raeen_linuxkpi::dma_buf::{
    self, DmaBuf as LkDmaBuf, DmaBufAttachOps, DmaBufAttachment, DmaBufOps,
};

static EXP_ATTACH: AtomicU32 = AtomicU32::new(0);
static EXP_MAP: AtomicU32 = AtomicU32::new(0);
static EXP_PIN: AtomicU32 = AtomicU32::new(0);
static EXP_UNPIN: AtomicU32 = AtomicU32::new(0);
static IMP_MOVE: AtomicU32 = AtomicU32::new(0);
static mut MOCK_SGT: SgTable = SgTable {
    sgl: std::ptr::null_mut(),
    nents: 7,
    orig_nents: 7,
};

extern "C" fn exp_attach(_d: *mut LkDmaBuf, _a: *mut DmaBufAttachment) -> i32 {
    EXP_ATTACH.fetch_add(1, Ordering::SeqCst);
    0
}
extern "C" fn exp_map(_a: *mut DmaBufAttachment, _dir: i32) -> *mut SgTable {
    EXP_MAP.fetch_add(1, Ordering::SeqCst);
    core::ptr::addr_of_mut!(MOCK_SGT)
}
extern "C" fn exp_unmap(_a: *mut DmaBufAttachment, _s: *mut SgTable, _dir: i32) {}
extern "C" fn exp_pin(_a: *mut DmaBufAttachment) -> i32 {
    EXP_PIN.fetch_add(1, Ordering::SeqCst);
    0
}
extern "C" fn exp_unpin(_a: *mut DmaBufAttachment) {
    EXP_UNPIN.fetch_add(1, Ordering::SeqCst);
}
extern "C" fn imp_move(_a: *mut DmaBufAttachment) {
    IMP_MOVE.fetch_add(1, Ordering::SeqCst);
}

fn test_dma_buf(h: &mut Harness) {
    unsafe {
        let ops = DmaBufOps {
            cache_sgt_mapping: false,
            attach: Some(exp_attach),
            detach: None,
            pin: Some(exp_pin),
            unpin: Some(exp_unpin),
            map_dma_buf: Some(exp_map),
            unmap_dma_buf: Some(exp_unmap),
            release: None,
            begin_cpu_access: None,
            end_cpu_access: None,
            mmap: None,
            vmap: None,
            vunmap: None,
        };
        let mut buf: LkDmaBuf = std::mem::zeroed();
        buf.ops = &ops as *const DmaBufOps;
        let bp = &mut buf as *mut LkDmaBuf;

        let iops = DmaBufAttachOps {
            allow_peer2peer: false,
            move_notify: Some(imp_move),
        };

        // dynamic_attach dispatches exporter attach + links the attachment
        EXP_ATTACH.store(0, Ordering::SeqCst);
        let at =
            dma_buf::dma_buf_dynamic_attach(bp, std::ptr::null_mut(), &iops, std::ptr::null_mut());
        let ok = (at as isize) > 0; // not an ERR_PTR
        h.check(
            "dma_buf: dynamic_attach ok + exporter attach ran",
            ok && EXP_ATTACH.load(Ordering::SeqCst) == 1,
        );

        // map dispatches to exporter map_dma_buf and caches sgt
        EXP_MAP.store(0, Ordering::SeqCst);
        let sgt = dma_buf::dma_buf_map_attachment(at, 2);
        h.check(
            "dma_buf: map dispatches to exporter",
            sgt == core::ptr::addr_of_mut!(MOCK_SGT)
                && EXP_MAP.load(Ordering::SeqCst) == 1
                && (*at).sgt == sgt,
        );
        dma_buf::dma_buf_unmap_attachment(at, sgt, 2);
        h.check("dma_buf: unmap clears cached sgt", (*at).sgt.is_null());

        // pin/unpin dispatch
        EXP_PIN.store(0, Ordering::SeqCst);
        EXP_UNPIN.store(0, Ordering::SeqCst);
        h.check(
            "dma_buf: pin dispatches (ret 0)",
            dma_buf::dma_buf_pin(at) == 0 && EXP_PIN.load(Ordering::SeqCst) == 1,
        );
        dma_buf::dma_buf_unpin(at);
        h.check(
            "dma_buf: unpin dispatches",
            EXP_UNPIN.load(Ordering::SeqCst) == 1,
        );

        // move_notify reaches the importer
        IMP_MOVE.store(0, Ordering::SeqCst);
        dma_buf::dma_buf_move_notify(bp);
        h.check(
            "dma_buf: move_notify reaches importer",
            IMP_MOVE.load(Ordering::SeqCst) == 1,
        );

        dma_buf::dma_buf_detach(bp, at);

        // get(fd) reports the documented host-brokering limitation (ERR_PTR)
        let g = dma_buf::dma_buf_get(3);
        h.check(
            "dma_buf: get(fd) is ERR_PTR (host brokering pending)",
            (g as isize) == -19,
        );
    }
}

// ── amdgpu stage-6 SOC15 offsets (discovery-driven) ───────────────────────────
// With IP discovery supplied, init_rings must program the CP ring via the gfx11
// SOC15 offsets (GC_base + reg)<<2, NOT gc11's legacy GCN offsets — the suspected
// CP-ring readback-mismatch fix. FAIL-able: asserts the SOC15 key was written and
// the legacy key was not.
fn test_amdgpu_soc15_offsets(h: &mut Harness) {
    const GC_BASE: u32 = 0x8000; // GC segment 0 (CP_RB0_* registers)
    const GC_BASE_SEG1: u32 = 0x9000; // GC segment 1 (CP_ME_CNTL, RLC_SAFE_MODE)
                                      // The real gfx11 GFX-CP halt mask (ME_HALT|PFP_HALT) from the driver.
    const HALT_MASK: u32 = raeen_amdgpu::gc11::CP_ME_CNTL_GFX11_HALT_MASK;
    let mut mock = MockGpu::new(true, true, None);
    // Discovery publishes a GC block with BOTH base segments (real GC blocks do;
    // CP_ME_CNTL is base_idx 1, so seg 1 must be present for gfx_regs to resolve).
    mock.gfx_discovery = Some(vec![raeen_amdgpu::discovery::IpBlock {
        hw_id: raeen_amdgpu::regs::GC_HWID,
        instance: 0,
        bases: vec![GC_BASE, GC_BASE_SEG1],
    }]);
    // Model a CP the GOP left HALTED + a confirmed halt mask so cp_gfx_enable runs.
    let cp_me_cntl = (GC_BASE_SEG1 + 0x803) << 2; // regCP_ME_CNTL (base_idx 1)
    mock.regs.insert(cp_me_cntl, HALT_MASK);
    mock.cp_me_cntl_halt_mask = Some(HALT_MASK);
    // Model BOTH engines posting their fences on submit, so init_rings' CP and
    // SDMA fence-polls complete. init_rings' DMA allocs from this mock's
    // 0x1_0000_0000 base, in order: gfx 64K @..0000, sdma_ring 64K @..1_0000,
    // cp_fence_buf 4K @..2_0000, sdma_scratch 4K @..2_1000, sdma_fence_buf 4K
    // @..2_2000. CP WPTR reg = GC seg0 + 0x1df4; SDMA WPTR reg = GC seg0 + 0x85.
    const CP_FENCE_ADDR: u64 = 0x1_0002_0000;
    const SDMA_FENCE_ADDR: u64 = 0x1_0002_2000;
    let cp_wptr = (GC_BASE + 0x1df4) << 2;
    let sdma_wptr = (GC_BASE + 0x85) << 2;
    mock.complete_fence = vec![(cp_wptr, CP_FENCE_ADDR, 1), (sdma_wptr, SDMA_FENCE_ADDR, 1)];
    let dev = bringup::Device {
        handle: 0x42,
        vendor: bringup::AMD_VENDOR,
        device: bringup::RADEON_760M,
        vram_base: 0x1_0000_0000,
        vram_size: 2048 * 1024 * 1024,
        bootup_sclk_mhz: 0,
        bootup_mclk_mhz: 0,
    };
    let ok = bringup::init_rings(&mut mock, &dev);
    h.check("amdgpu soc15: init_rings ok with discovery", ok);
    let soc15_base = (GC_BASE + 0x1de0) << 2; // regCP_RB0_BASE
    h.check(
        "amdgpu soc15: CP_RB0_BASE programmed at SOC15 offset",
        mock.regs.contains_key(&soc15_base),
    );
    h.check(
        "amdgpu soc15: legacy gc11 CP offset NOT used",
        !mock.regs.contains_key(&gc11::MM_CP_RB0_BASE),
    );
    // CP enable: stage 6 released the CP from halt (halt bits cleared in CP_ME_CNTL).
    h.check(
        "amdgpu soc15: CP unhalted via CP_ME_CNTL",
        mock.regs.get(&cp_me_cntl) == Some(&0),
    );
    // SDMA ring: stage 6 programmed + submitted the SDMA0 QUEUE0 queue. RB_CNTL
    // (GC seg0 + 0x80) must have RB_ENABLE set and WPTR (+0x85) must be advanced.
    let sdma_rb_cntl = (GC_BASE + 0x80) << 2;
    let sdma_rb_wptr = (GC_BASE + 0x85) << 2;
    h.check(
        "amdgpu soc15: SDMA RB_CNTL has RB_ENABLE",
        mock.regs.get(&sdma_rb_cntl).copied().unwrap_or(0)
            & raeen_amdgpu::sdma::SDMA_RB_CNTL_RB_ENABLE
            != 0,
    );
    h.check(
        "amdgpu soc15: SDMA ring submitted (WPTR advanced)",
        mock.regs.get(&sdma_rb_wptr).copied().unwrap_or(0) != 0,
    );
    // Fence-polls: the modeled engines posted their fences into the fence
    // buffers, so init_rings observed completion for BOTH the GFX CP (RELEASE_MEM)
    // and the SDMA engine end to end through stage 6 — the "the engine actually
    // ran the submitted work" proof, not just "it was submitted".
    let cp_done = mock
        .dma
        .values()
        .any(|(addr, c)| *addr == CP_FENCE_ADDR && c.first() == Some(&1));
    h.check(
        "amdgpu soc15: CP fence posted (CP executed via poll)",
        cp_done,
    );
    let sdma_done = mock
        .dma
        .values()
        .any(|(addr, c)| *addr == SDMA_FENCE_ADDR && c.first() == Some(&1));
    h.check(
        "amdgpu soc15: SDMA fence posted (fill complete via poll)",
        sdma_done,
    );
}

fn main() {
    // GPU-only mode (`xtask gpu-test` sets RAEEN_GPU_ONLY): run ONLY the pure
    // mock-GPU amdgpu bring-up tests, with the `[amdgpu]` stage transcript on.
    // The other facades reach `host::sys_*`, which issues raw `syscall`
    // instructions — harmless on Linux, a hard fault on a non-Linux host — so
    // this focused path stays crash-free on any dev box while still replaying
    // amdgpud's ACTUAL bring-up sequence against the hardware-reaction mock.
    if std::env::var("RAEEN_GPU_ONLY").is_ok() {
        println!("[linuxkpi-harness] GPU-only: amdgpu bring-up on a mock GPU (host, no QEMU/iron)");
        let mut h = Harness::new();
        test_real_amdgpu_bringup(&mut h);
        test_amdgpu_submit_fence(&mut h);
        test_amdgpu_soc15_offsets(&mut h);
        h.finish(); // exits the process (PASS -> 0, any FAIL -> 1)
    }

    println!("[linuxkpi-harness] raeen_linuxkpi logic + mock-GPU bring-up (host, no QEMU)");
    let mut h = Harness::new();
    test_atomics(&mut h);
    test_bitops(&mut h);
    test_mmio(&mut h);
    test_alloc(&mut h);
    test_heap_free(&mut h);
    test_locks(&mut h);
    test_printk_format(&mut h);
    test_driver_bringup(&mut h);
    test_real_amdgpu_bringup(&mut h);
    test_amdgpu_submit_fence(&mut h);
    test_amdgpu_soc15_offsets(&mut h);
    test_workqueue(&mut h);
    test_scatterlist(&mut h);
    test_refcount(&mut h);
    test_idr(&mut h);
    test_dma_pool(&mut h);
    test_kfifo(&mut h);
    test_bitmap(&mut h);
    test_printf(&mut h);
    test_kstrtox(&mut h);
    test_sync_extras(&mut h);
    test_slab(&mut h);
    test_pci_ext_cap(&mut h);
    test_dma_fence(&mut h);
    test_dma_fence_array(&mut h);
    test_dma_fence_chain(&mut h);
    test_dma_resv(&mut h);
    test_dma_buf(&mut h);
    h.finish();
}
