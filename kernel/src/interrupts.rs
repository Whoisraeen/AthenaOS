//! Interrupt Descriptor Table (IDT) and interrupt handlers.
//!
//! Handles CPU exceptions, hardware interrupts via IOAPIC, and
//! dynamically-allocated MSI/MSI-X vectors (48–79).
//!
//! **Alignment fix (BUILD_CODEGEN_BUG.md):** Every handler uses the
//! naked→inner pattern: a `#[unsafe(naked)] extern "C"` asm stub that
//! saves GPRs, 16-byte-aligns RSP, and calls an `#[inline(never)]`
//! normal-ABI inner function.  This sidesteps the LLVM `x86-interrupt`
//! ABI misalignment bug where the MC assembler rejects 16-byte-aligned
//! spills on the 8-byte-aligned interrupt stack (no-error-code case).

use crate::gdt;
use crate::serial_println;
use core::sync::atomic::{AtomicU64, Ordering};
use lazy_static::lazy_static;
use x86_64::structures::idt::InterruptDescriptorTable;

pub const PIC_1_OFFSET: u8 = 32;

// ── Dynamic MSI vector dispatch ──────────────────────────────────────────

/// First IDT vector available for MSI/MSI-X allocation.
pub const MSI_VEC_BASE: u8 = 64;
/// How many dynamic vectors we support (64..255 inclusive, which is 192 available vectors).
pub const MSI_VEC_COUNT: usize = 192;

/// Function-pointer dispatch table for dynamically-registered handlers.
/// Index 0 corresponds to vector MSI_VEC_BASE, index 1 to MSI_VEC_BASE+1, etc.
static MSI_DISPATCH: [AtomicU64; MSI_VEC_COUNT] = {
    const ZERO: AtomicU64 = AtomicU64::new(0);
    [ZERO; MSI_VEC_COUNT]
};

fn dispatch_msi(idx: usize) {
    let addr = MSI_DISPATCH[idx].load(Ordering::Acquire);
    if addr != 0 {
        let f: fn() = unsafe { core::mem::transmute(addr as usize) };
        f();
    }

    // Push the event to AER for user-space drivers (Split-Driver fast path).
    let vector = (MSI_VEC_BASE as usize + idx) as u32;
    let _ = crate::linux_compat::dispatch_irq(vector);
    crate::aer::dispatch_irq(vector, 0);

    // LinuxKPI daemons blocked on SYS_LINUXKPI_IRQ_WAIT / SYS_IRQ_WAIT.
    crate::linuxkpi_host::lkpi_deliver_irq(vector as u8);

    crate::arch::interrupt_controller::eoi();
}

static MSI_ALLOC_BITMAP: [AtomicU64; 3] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];

pub fn allocate_msi_vector(handler: fn()) -> Option<u8> {
    for (bucket_idx, bucket) in MSI_ALLOC_BITMAP.iter().enumerate() {
        let mut current = bucket.load(Ordering::Relaxed);
        loop {
            let free_bit = (!current).trailing_zeros() as usize;
            if free_bit >= 64 {
                break; // This bucket is completely full, move to next
            }

            let target_mask = 1 << free_bit;
            let new_value = current | target_mask;

            match bucket.compare_exchange_weak(
                current,
                new_value,
                Ordering::SeqCst,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    // BUG-06: compute the IDT vector in usize and bounds-check
                    // before narrowing. The u8 expression happens to fit for the
                    // current 3 buckets (max 64+128+63=255), but grows to a
                    // silent wrap the moment MSI_VEC_COUNT is increased.
                    let vector_usize = MSI_VEC_BASE as usize + bucket_idx * 64 + free_bit;
                    debug_assert!(
                        vector_usize <= u8::MAX as usize,
                        "MSI vector {} overflows u8",
                        vector_usize
                    );
                    let vector = vector_usize as u8;
                    register_handler(vector, handler);
                    return Some(vector);
                }
                Err(actual) => current = actual, // Bit modified by another core, retry loop
            }
        }
    }
    None // All 192 vectors exhausted
}

/// Register a handler for a dynamically-allocated interrupt vector.
/// The vector must be in `[MSI_VEC_BASE, MSI_VEC_BASE + MSI_VEC_COUNT)`.
pub fn register_handler(vector: u8, handler: fn()) {
    let idx = vector
        .checked_sub(MSI_VEC_BASE)
        .expect("vector below MSI_VEC_BASE") as usize;
    assert!(idx < MSI_VEC_COUNT, "vector {} out of MSI range", vector);
    MSI_DISPATCH[idx].store(handler as usize as u64, Ordering::Release);
}

// ── MSI naked stubs ─────────────────────────────────────────────────────
//
// Each MSI stub is a naked function that saves all GPRs, aligns the stack,
// calls dispatch_msi(idx), then restores and iretq.

macro_rules! msi_naked_stub {
    ($name:ident, $idx:expr) => {
        #[unsafe(naked)]
        extern "C" fn $name() {
            core::arch::naked_asm!(
                // Save all general-purpose registers
                "push r15",
                "push r14",
                "push r13",
                "push r12",
                "push r11",
                "push r10",
                "push r9",
                "push r8",
                "push rdi",
                "push rsi",
                "push rbp",
                "push rbx",
                "push rdx",
                "push rcx",
                "push rax",
                // 16-byte-align RSP (save old RSP in r15 which is already saved)
                "mov rbp, rsp",
                "and rsp, -16",
                // Call the inner function with the MSI index
                "mov edi, {idx}",
                "call {inner}",
                // Restore original RSP and pop GPRs
                "mov rsp, rbp",
                "pop rax",
                "pop rcx",
                "pop rdx",
                "pop rbx",
                "pop rbp",
                "pop rsi",
                "pop rdi",
                "pop r8",
                "pop r9",
                "pop r10",
                "pop r11",
                "pop r12",
                "pop r13",
                "pop r14",
                "pop r15",
                "iretq",
                idx = const $idx,
                inner = sym msi_inner,
            );
        }
    };
}

#[inline(never)]
#[no_mangle]
extern "C" fn msi_inner(idx: usize) {
    dispatch_msi(idx);
}

msi_naked_stub!(msi_stub_000, 0);
msi_naked_stub!(msi_stub_001, 1);
msi_naked_stub!(msi_stub_002, 2);
msi_naked_stub!(msi_stub_003, 3);
msi_naked_stub!(msi_stub_004, 4);
msi_naked_stub!(msi_stub_005, 5);
msi_naked_stub!(msi_stub_006, 6);
msi_naked_stub!(msi_stub_007, 7);
msi_naked_stub!(msi_stub_008, 8);
msi_naked_stub!(msi_stub_009, 9);
msi_naked_stub!(msi_stub_010, 10);
msi_naked_stub!(msi_stub_011, 11);
msi_naked_stub!(msi_stub_012, 12);
msi_naked_stub!(msi_stub_013, 13);
msi_naked_stub!(msi_stub_014, 14);
msi_naked_stub!(msi_stub_015, 15);
msi_naked_stub!(msi_stub_016, 16);
msi_naked_stub!(msi_stub_017, 17);
msi_naked_stub!(msi_stub_018, 18);
msi_naked_stub!(msi_stub_019, 19);
msi_naked_stub!(msi_stub_020, 20);
msi_naked_stub!(msi_stub_021, 21);
msi_naked_stub!(msi_stub_022, 22);
msi_naked_stub!(msi_stub_023, 23);
msi_naked_stub!(msi_stub_024, 24);
msi_naked_stub!(msi_stub_025, 25);
msi_naked_stub!(msi_stub_026, 26);
msi_naked_stub!(msi_stub_027, 27);
msi_naked_stub!(msi_stub_028, 28);
msi_naked_stub!(msi_stub_029, 29);
msi_naked_stub!(msi_stub_030, 30);
msi_naked_stub!(msi_stub_031, 31);
msi_naked_stub!(msi_stub_032, 32);
msi_naked_stub!(msi_stub_033, 33);
msi_naked_stub!(msi_stub_034, 34);
msi_naked_stub!(msi_stub_035, 35);
msi_naked_stub!(msi_stub_036, 36);
msi_naked_stub!(msi_stub_037, 37);
msi_naked_stub!(msi_stub_038, 38);
msi_naked_stub!(msi_stub_039, 39);
msi_naked_stub!(msi_stub_040, 40);
msi_naked_stub!(msi_stub_041, 41);
msi_naked_stub!(msi_stub_042, 42);
msi_naked_stub!(msi_stub_043, 43);
msi_naked_stub!(msi_stub_044, 44);
msi_naked_stub!(msi_stub_045, 45);
msi_naked_stub!(msi_stub_046, 46);
msi_naked_stub!(msi_stub_047, 47);
msi_naked_stub!(msi_stub_048, 48);
msi_naked_stub!(msi_stub_049, 49);
msi_naked_stub!(msi_stub_050, 50);
msi_naked_stub!(msi_stub_051, 51);
msi_naked_stub!(msi_stub_052, 52);
msi_naked_stub!(msi_stub_053, 53);
msi_naked_stub!(msi_stub_054, 54);
msi_naked_stub!(msi_stub_055, 55);
msi_naked_stub!(msi_stub_056, 56);
msi_naked_stub!(msi_stub_057, 57);
msi_naked_stub!(msi_stub_058, 58);
msi_naked_stub!(msi_stub_059, 59);
msi_naked_stub!(msi_stub_060, 60);
msi_naked_stub!(msi_stub_061, 61);
msi_naked_stub!(msi_stub_062, 62);
msi_naked_stub!(msi_stub_063, 63);
msi_naked_stub!(msi_stub_064, 64);
msi_naked_stub!(msi_stub_065, 65);
msi_naked_stub!(msi_stub_066, 66);
msi_naked_stub!(msi_stub_067, 67);
msi_naked_stub!(msi_stub_068, 68);
msi_naked_stub!(msi_stub_069, 69);
msi_naked_stub!(msi_stub_070, 70);
msi_naked_stub!(msi_stub_071, 71);
msi_naked_stub!(msi_stub_072, 72);
msi_naked_stub!(msi_stub_073, 73);
msi_naked_stub!(msi_stub_074, 74);
msi_naked_stub!(msi_stub_075, 75);
msi_naked_stub!(msi_stub_076, 76);
msi_naked_stub!(msi_stub_077, 77);
msi_naked_stub!(msi_stub_078, 78);
msi_naked_stub!(msi_stub_079, 79);
msi_naked_stub!(msi_stub_080, 80);
msi_naked_stub!(msi_stub_081, 81);
msi_naked_stub!(msi_stub_082, 82);
msi_naked_stub!(msi_stub_083, 83);
msi_naked_stub!(msi_stub_084, 84);
msi_naked_stub!(msi_stub_085, 85);
msi_naked_stub!(msi_stub_086, 86);
msi_naked_stub!(msi_stub_087, 87);
msi_naked_stub!(msi_stub_088, 88);
msi_naked_stub!(msi_stub_089, 89);
msi_naked_stub!(msi_stub_090, 90);
msi_naked_stub!(msi_stub_091, 91);
msi_naked_stub!(msi_stub_092, 92);
msi_naked_stub!(msi_stub_093, 93);
msi_naked_stub!(msi_stub_094, 94);
msi_naked_stub!(msi_stub_095, 95);
msi_naked_stub!(msi_stub_096, 96);
msi_naked_stub!(msi_stub_097, 97);
msi_naked_stub!(msi_stub_098, 98);
msi_naked_stub!(msi_stub_099, 99);
msi_naked_stub!(msi_stub_100, 100);
msi_naked_stub!(msi_stub_101, 101);
msi_naked_stub!(msi_stub_102, 102);
msi_naked_stub!(msi_stub_103, 103);
msi_naked_stub!(msi_stub_104, 104);
msi_naked_stub!(msi_stub_105, 105);
msi_naked_stub!(msi_stub_106, 106);
msi_naked_stub!(msi_stub_107, 107);
msi_naked_stub!(msi_stub_108, 108);
msi_naked_stub!(msi_stub_109, 109);
msi_naked_stub!(msi_stub_110, 110);
msi_naked_stub!(msi_stub_111, 111);
msi_naked_stub!(msi_stub_112, 112);
msi_naked_stub!(msi_stub_113, 113);
msi_naked_stub!(msi_stub_114, 114);
msi_naked_stub!(msi_stub_115, 115);
msi_naked_stub!(msi_stub_116, 116);
msi_naked_stub!(msi_stub_117, 117);
msi_naked_stub!(msi_stub_118, 118);
msi_naked_stub!(msi_stub_119, 119);
msi_naked_stub!(msi_stub_120, 120);
msi_naked_stub!(msi_stub_121, 121);
msi_naked_stub!(msi_stub_122, 122);
msi_naked_stub!(msi_stub_123, 123);
msi_naked_stub!(msi_stub_124, 124);
msi_naked_stub!(msi_stub_125, 125);
msi_naked_stub!(msi_stub_126, 126);
msi_naked_stub!(msi_stub_127, 127);
msi_naked_stub!(msi_stub_128, 128);
msi_naked_stub!(msi_stub_129, 129);
msi_naked_stub!(msi_stub_130, 130);
msi_naked_stub!(msi_stub_131, 131);
msi_naked_stub!(msi_stub_132, 132);
msi_naked_stub!(msi_stub_133, 133);
msi_naked_stub!(msi_stub_134, 134);
msi_naked_stub!(msi_stub_135, 135);
msi_naked_stub!(msi_stub_136, 136);
msi_naked_stub!(msi_stub_137, 137);
msi_naked_stub!(msi_stub_138, 138);
msi_naked_stub!(msi_stub_139, 139);
msi_naked_stub!(msi_stub_140, 140);
msi_naked_stub!(msi_stub_141, 141);
msi_naked_stub!(msi_stub_142, 142);
msi_naked_stub!(msi_stub_143, 143);
msi_naked_stub!(msi_stub_144, 144);
msi_naked_stub!(msi_stub_145, 145);
msi_naked_stub!(msi_stub_146, 146);
msi_naked_stub!(msi_stub_147, 147);
msi_naked_stub!(msi_stub_148, 148);
msi_naked_stub!(msi_stub_149, 149);
msi_naked_stub!(msi_stub_150, 150);
msi_naked_stub!(msi_stub_151, 151);
msi_naked_stub!(msi_stub_152, 152);
msi_naked_stub!(msi_stub_153, 153);
msi_naked_stub!(msi_stub_154, 154);
msi_naked_stub!(msi_stub_155, 155);
msi_naked_stub!(msi_stub_156, 156);
msi_naked_stub!(msi_stub_157, 157);
msi_naked_stub!(msi_stub_158, 158);
msi_naked_stub!(msi_stub_159, 159);
msi_naked_stub!(msi_stub_160, 160);
msi_naked_stub!(msi_stub_161, 161);
msi_naked_stub!(msi_stub_162, 162);
msi_naked_stub!(msi_stub_163, 163);
msi_naked_stub!(msi_stub_164, 164);
msi_naked_stub!(msi_stub_165, 165);
msi_naked_stub!(msi_stub_166, 166);
msi_naked_stub!(msi_stub_167, 167);
msi_naked_stub!(msi_stub_168, 168);
msi_naked_stub!(msi_stub_169, 169);
msi_naked_stub!(msi_stub_170, 170);
msi_naked_stub!(msi_stub_171, 171);
msi_naked_stub!(msi_stub_172, 172);
msi_naked_stub!(msi_stub_173, 173);
msi_naked_stub!(msi_stub_174, 174);
msi_naked_stub!(msi_stub_175, 175);
msi_naked_stub!(msi_stub_176, 176);
msi_naked_stub!(msi_stub_177, 177);
msi_naked_stub!(msi_stub_178, 178);
msi_naked_stub!(msi_stub_179, 179);
msi_naked_stub!(msi_stub_180, 180);
msi_naked_stub!(msi_stub_181, 181);
msi_naked_stub!(msi_stub_182, 182);
msi_naked_stub!(msi_stub_183, 183);
msi_naked_stub!(msi_stub_184, 184);
msi_naked_stub!(msi_stub_185, 185);
msi_naked_stub!(msi_stub_186, 186);
msi_naked_stub!(msi_stub_187, 187);
msi_naked_stub!(msi_stub_188, 188);
msi_naked_stub!(msi_stub_189, 189);
msi_naked_stub!(msi_stub_190, 190);
msi_naked_stub!(msi_stub_191, 191);

/// Raw addresses for IDT registration via set_handler_addr.
const MSI_STUBS: [unsafe extern "C" fn(); MSI_VEC_COUNT] = [
    msi_stub_000,
    msi_stub_001,
    msi_stub_002,
    msi_stub_003,
    msi_stub_004,
    msi_stub_005,
    msi_stub_006,
    msi_stub_007,
    msi_stub_008,
    msi_stub_009,
    msi_stub_010,
    msi_stub_011,
    msi_stub_012,
    msi_stub_013,
    msi_stub_014,
    msi_stub_015,
    msi_stub_016,
    msi_stub_017,
    msi_stub_018,
    msi_stub_019,
    msi_stub_020,
    msi_stub_021,
    msi_stub_022,
    msi_stub_023,
    msi_stub_024,
    msi_stub_025,
    msi_stub_026,
    msi_stub_027,
    msi_stub_028,
    msi_stub_029,
    msi_stub_030,
    msi_stub_031,
    msi_stub_032,
    msi_stub_033,
    msi_stub_034,
    msi_stub_035,
    msi_stub_036,
    msi_stub_037,
    msi_stub_038,
    msi_stub_039,
    msi_stub_040,
    msi_stub_041,
    msi_stub_042,
    msi_stub_043,
    msi_stub_044,
    msi_stub_045,
    msi_stub_046,
    msi_stub_047,
    msi_stub_048,
    msi_stub_049,
    msi_stub_050,
    msi_stub_051,
    msi_stub_052,
    msi_stub_053,
    msi_stub_054,
    msi_stub_055,
    msi_stub_056,
    msi_stub_057,
    msi_stub_058,
    msi_stub_059,
    msi_stub_060,
    msi_stub_061,
    msi_stub_062,
    msi_stub_063,
    msi_stub_064,
    msi_stub_065,
    msi_stub_066,
    msi_stub_067,
    msi_stub_068,
    msi_stub_069,
    msi_stub_070,
    msi_stub_071,
    msi_stub_072,
    msi_stub_073,
    msi_stub_074,
    msi_stub_075,
    msi_stub_076,
    msi_stub_077,
    msi_stub_078,
    msi_stub_079,
    msi_stub_080,
    msi_stub_081,
    msi_stub_082,
    msi_stub_083,
    msi_stub_084,
    msi_stub_085,
    msi_stub_086,
    msi_stub_087,
    msi_stub_088,
    msi_stub_089,
    msi_stub_090,
    msi_stub_091,
    msi_stub_092,
    msi_stub_093,
    msi_stub_094,
    msi_stub_095,
    msi_stub_096,
    msi_stub_097,
    msi_stub_098,
    msi_stub_099,
    msi_stub_100,
    msi_stub_101,
    msi_stub_102,
    msi_stub_103,
    msi_stub_104,
    msi_stub_105,
    msi_stub_106,
    msi_stub_107,
    msi_stub_108,
    msi_stub_109,
    msi_stub_110,
    msi_stub_111,
    msi_stub_112,
    msi_stub_113,
    msi_stub_114,
    msi_stub_115,
    msi_stub_116,
    msi_stub_117,
    msi_stub_118,
    msi_stub_119,
    msi_stub_120,
    msi_stub_121,
    msi_stub_122,
    msi_stub_123,
    msi_stub_124,
    msi_stub_125,
    msi_stub_126,
    msi_stub_127,
    msi_stub_128,
    msi_stub_129,
    msi_stub_130,
    msi_stub_131,
    msi_stub_132,
    msi_stub_133,
    msi_stub_134,
    msi_stub_135,
    msi_stub_136,
    msi_stub_137,
    msi_stub_138,
    msi_stub_139,
    msi_stub_140,
    msi_stub_141,
    msi_stub_142,
    msi_stub_143,
    msi_stub_144,
    msi_stub_145,
    msi_stub_146,
    msi_stub_147,
    msi_stub_148,
    msi_stub_149,
    msi_stub_150,
    msi_stub_151,
    msi_stub_152,
    msi_stub_153,
    msi_stub_154,
    msi_stub_155,
    msi_stub_156,
    msi_stub_157,
    msi_stub_158,
    msi_stub_159,
    msi_stub_160,
    msi_stub_161,
    msi_stub_162,
    msi_stub_163,
    msi_stub_164,
    msi_stub_165,
    msi_stub_166,
    msi_stub_167,
    msi_stub_168,
    msi_stub_169,
    msi_stub_170,
    msi_stub_171,
    msi_stub_172,
    msi_stub_173,
    msi_stub_174,
    msi_stub_175,
    msi_stub_176,
    msi_stub_177,
    msi_stub_178,
    msi_stub_179,
    msi_stub_180,
    msi_stub_181,
    msi_stub_182,
    msi_stub_183,
    msi_stub_184,
    msi_stub_185,
    msi_stub_186,
    msi_stub_187,
    msi_stub_188,
    msi_stub_189,
    msi_stub_190,
    msi_stub_191,
];

pub unsafe fn disable_pic() {
    use x86_64::instructions::port::Port;
    let mut a1: Port<u8> = Port::new(0xA1);
    let mut a2: Port<u8> = Port::new(0x21);
    a1.write(0xFF);
    a2.write(0xFF);
}

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = PIC_1_OFFSET,
    Keyboard,
    Mouse = 44,
    VirtioBlk = 43,
    VirtioNet = 42,
    Sci = 0xF0,
    Spurious = 0xFF,
    ApicError = 0xFE,
}

impl InterruptIndex {
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        // All handlers are now naked stubs registered via set_handler_addr
        // to avoid the LLVM x86-interrupt stack alignment codegen bug.
        unsafe {
            idt.breakpoint.set_handler_addr(
                crate::arch::VirtAddr::new(breakpoint_handler as *const () as u64));
            idt.double_fault
                .set_handler_addr(
                    crate::arch::VirtAddr::new(double_fault_handler as *const () as u64))
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
            idt.page_fault.set_handler_addr(
                crate::arch::VirtAddr::new(page_fault_handler as *const () as u64));
            idt.general_protection_fault.set_handler_addr(
                crate::arch::VirtAddr::new(gpf_handler as *const () as u64));
            idt.stack_segment_fault.set_handler_addr(
                crate::arch::VirtAddr::new(ssf_handler as *const () as u64));
            idt.segment_not_present.set_handler_addr(
                crate::arch::VirtAddr::new(snp_handler as *const () as u64));
            idt.invalid_opcode.set_handler_addr(
                crate::arch::VirtAddr::new(ud_handler as *const () as u64));
            idt.machine_check.set_handler_addr(
                crate::arch::VirtAddr::new(machine_check_handler as *const () as u64));
            idt[InterruptIndex::Timer.as_u8()].set_handler_addr(
                crate::arch::VirtAddr::new(timer_interrupt_handler as *const () as u64));
            idt[InterruptIndex::Keyboard.as_u8()].set_handler_addr(
                crate::arch::VirtAddr::new(keyboard_interrupt_handler as *const () as u64));
            idt[InterruptIndex::Mouse.as_u8()].set_handler_addr(
                crate::arch::VirtAddr::new(mouse_interrupt_handler as *const () as u64));
            idt[InterruptIndex::VirtioBlk.as_u8()].set_handler_addr(
                crate::arch::VirtAddr::new(virtio_blk_interrupt_handler as *const () as u64));
            idt[InterruptIndex::VirtioNet.as_u8()].set_handler_addr(
                crate::arch::VirtAddr::new(virtio_net_interrupt_handler as *const () as u64));
            idt[InterruptIndex::Spurious.as_u8()].set_handler_addr(
                crate::arch::VirtAddr::new(spurious_interrupt_handler as *const () as u64));
            idt[InterruptIndex::ApicError.as_u8()].set_handler_addr(
                crate::arch::VirtAddr::new(apic_error_handler as *const () as u64));
            idt[InterruptIndex::Sci.as_u8()].set_handler_addr(
                crate::arch::VirtAddr::new(sci_interrupt_handler as *const () as u64));
            // Wire up all 192 MSI dynamic-dispatch stubs.
            {
                let mut i: u8 = 0;
                while (i as usize) < MSI_VEC_COUNT {
                    idt[MSI_VEC_BASE + i].set_handler_addr(
                        crate::arch::VirtAddr::new(MSI_STUBS[i as usize] as *const () as u64));
                    i += 1;
                }
            }
        }
        idt
    };
}

pub fn init_idt() {
    IDT.load();
}

// ═══════════════════════════════════════════════════════════════════════════
// Naked asm stubs + inner functions for all interrupt handlers.
//
// Pattern: The naked stub saves all 15 GPRs (120 bytes), then aligns RSP
// to 16 bytes (required by SysV ABI for the `call`), calls the inner fn,
// restores RSP + GPRs, and does `iretq`.
//
// For NO-ERROR-CODE interrupts the CPU pushes 5 qwords (40 bytes).
// After our 15 pushes the stack is at 40 + 120 = 160 = 16*10 (aligned).
// We still AND to -16 for paranoia / future-proofing.
//
// For ERROR-CODE interrupts the CPU pushes 6 qwords (48 bytes) including
// the error code.  The stub pops the error code into rdi (arg1) before
// saving GPRs, then passes it to the inner function.
// ═══════════════════════════════════════════════════════════════════════════

/// Shared handler tail for faults that are usually caused by buggy *user*
/// code (invalid opcode, #GP, stack-segment, segment-not-present). If the
/// saved code segment has RPL=3 (Ring-3) and a task is running, kill that task
/// and reschedule instead of hanging the CPU. A Ring-0 fault is a real kernel
/// bug → panic.
#[inline(never)]
fn kill_user_task_or_panic(label: &str, detail: u64, saved_cs: u64) -> ! {
    let cpu_id = crate::gdt::current_cpu_id();
    serial_println!(
        "[EXCEPTION] {} cpu={} detail={:#x} cs={:#x}",
        label,
        cpu_id,
        detail,
        saved_cs,
    );
    let from_user = (saved_cs & 0x3) == 0x3;
    if from_user && crate::scheduler::has_current_task() {
        serial_println!("[EXCEPTION] killing faulting user task and rescheduling");
        crate::scheduler::exit_current_task(0xDEAD_C0DE_u64); // does not return
    }
    panic!(
        "{} in Ring-0 (kernel bug): detail={:#x} cs={:#x}",
        label, detail, saved_cs
    );
}

// ── Breakpoint (no error code) ──────────────────────────────────────────

#[unsafe(naked)]
extern "C" fn breakpoint_handler() {
    core::arch::naked_asm!(
        "push r15", "push r14", "push r13", "push r12",
        "push r11", "push r10", "push r9",  "push r8",
        "push rdi", "push rsi", "push rbp", "push rbx",
        "push rdx", "push rcx", "push rax",
        "mov rbp, rsp",
        "and rsp, -16",
        "call {inner}",
        "mov rsp, rbp",
        "pop rax",  "pop rcx",  "pop rdx",  "pop rbx",
        "pop rbp",  "pop rsi",  "pop rdi",
        "pop r8",   "pop r9",   "pop r10",  "pop r11",
        "pop r12",  "pop r13",  "pop r14",  "pop r15",
        "iretq",
        inner = sym breakpoint_inner,
    );
}

#[inline(never)]
extern "C" fn breakpoint_inner() {
    serial_println!("[EXCEPTION] BREAKPOINT");
}

// ── Double Fault (error code pushed by CPU) ─────────────────────────────

#[unsafe(naked)]
extern "C" fn double_fault_handler() {
    // CPU pushes: SS, RSP, RFLAGS, CS, RIP, error_code
    // We pop the error code into rdi before saving GPRs.
    core::arch::naked_asm!(
        // Pop error code (we ignore it for double fault but must remove from stack)
        "pop rdi",
        "push r15", "push r14", "push r13", "push r12",
        "push r11", "push r10", "push r9",  "push r8",
        "push rdi", "push rsi", "push rbp", "push rbx",
        "push rdx", "push rcx", "push rax",
        "mov rbp, rsp",
        // Pass the GPR-frame base as arg2 so the inner fn can read the
        // CPU-pushed exception frame (RIP/CS/RFLAGS/RSP/SS) above the 15 saved
        // GPRs (at rbp + 120) — the faulting context the bare handler discarded.
        "mov rsi, rbp",
        "and rsp, -16",
        // rdi already has error_code
        "call {inner}",
        // double_fault_inner diverges (-> !), so we never get here,
        // but provide a halt loop just in case.
        "2:",
        "hlt",
        "jmp 2b",
        inner = sym double_fault_inner,
    );
}

#[inline(never)]
extern "C" fn double_fault_inner(_error_code: u64, frame_base: u64) -> ! {
    // Keep this minimal — we're on the IST stack and any allocation/formatting
    // that overflows it will triple-fault and reset silently.
    let cpu_id = crate::gdt::current_cpu_id();
    // The CPU-pushed exception frame sits above the 15 GPRs we saved: at
    // frame_base + 15*8. Read RIP/CS/RSP for diagnosis (plain volatile reads,
    // no allocation — IST-safe).
    let (rip, cs, rsp) = unsafe {
        let f = (frame_base + 120) as *const u64;
        (
            core::ptr::read_volatile(f),
            core::ptr::read_volatile(f.add(1)),
            core::ptr::read_volatile(f.add(3)),
        )
    };
    let cr2 = x86_64::registers::control::Cr2::read_raw();
    serial_println!(
        "[EXCEPTION] DOUBLE FAULT cpu={} rip={:#x} cs={:#x} rsp={:#x} cr2={:#x}",
        cpu_id,
        rip,
        cs,
        rsp,
        cr2
    );

    // Force-release every system-wide spinlock the dying CPU might be
    // holding. Concretely, a CPU that #DF'd inside
    //   * `Task::drop` → `memory::free_kernel_stack` → `PAGE_TABLE_LOCK`
    //   * `Box::new` / `Vec::push` → heap allocator's lock
    //   * any scheduler operation → `SCHEDULER`
    // would otherwise wedge every surviving CPU the next time it touches
    // those subsystems. The classic symptom this fixes: smp_worker_N
    // exits, #DF's mid-dealloc, and the BSP's later `spawn_*_thread`
    // call hangs forever in `alloc_kernel_stack` waiting on
    // `PAGE_TABLE_LOCK`. Comment on `force_unlock_scheduler` at
    // scheduler.rs already documented this contract — it just wasn't
    // wired up.
    //
    // SAFETY: we're about to enter `hlt_loop()`, never to return. The
    // forced unlock can never race with a legitimate re-acquire on
    // this CPU. Cross-CPU state the dying CPU was mid-mutating
    // (page-table walk, free-list edit) is left in whatever partial
    // shape the fault interrupted it in; surviving subsystems may
    // observe a leak or a stranded mapping but will not deadlock.
    unsafe {
        crate::scheduler::force_unlock_scheduler();
        crate::memory::force_unlock_page_table();
        crate::memory::allocator::force_unlock_heap();
    }
    serial_println!(
        "[EXCEPTION] cpu={} force-released SCHEDULER + PAGE_TABLE_LOCK + heap; entering hlt_loop",
        cpu_id,
    );

    // Best-effort bootlog flush so a bare-metal #DF leaves evidence on the
    // ESP (Athena has no serial; a #DF during boot was previously a silent
    // hang with an empty log). ONCE only — if the flush itself #DFs we must
    // not recurse — and only after the force-unlocks above so the allocator
    // and block stack are usable. Worst case it triple-faults and resets,
    // which loses no information we'd otherwise have kept.
    static DF_FLUSHED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    if !DF_FLUSHED.swap(true, core::sync::atomic::Ordering::SeqCst) {
        crate::bootlog_persist::flush();
    }

    crate::hlt_loop();
}

// ── General Protection Fault (error code pushed by CPU) ─────────────────

#[unsafe(naked)]
extern "C" fn gpf_handler() {
    // Save ALL GPRs BEFORE touching the error code. The previous prologue did
    // `pop rdi` (error code → arg1) first, so the interrupted context's rdi
    // was destroyed and the epilogue restored rdi = ERROR CODE. On the
    // recovered-return path (the armed-MSR #GP probe, error code 0) the task
    // resumed with rdi=0 — if the compiler had anything precious live in rdi
    // across the `rdmsr` (e.g. a struct-return pointer), the next write
    // through it was a wild near-NULL store. Bit as: rdmsr_safe(SPEC_CTRL/
    // ARCH_CAPABILITIES) on QEMU's qemu64 → #GP(0) → resume with rdi=0 →
    // cpu_features::current_cpu_security_descriptor stored its sret struct to
    // 0x0+0x25 → boot task killed → "no next task" panic (2026-07-08).
    // Layout after the 15 pushes: err@+120, RIP@+128, CS@+136.
    core::arch::naked_asm!(
        "push r15", "push r14", "push r13", "push r12",
        "push r11", "push r10", "push r9",  "push r8",
        "push rdi", "push rsi", "push rbp", "push rbx",
        "push rdx", "push rcx", "push rax",
        "mov rdi, [rsp + 120]",   // error code                    → arg1
        "mov rsi, [rsp + 136]",   // saved CS                      → arg2
        // arg3 (rdx): pointer to the saved-RIP slot on the iretq frame so
        // the fault-tolerant MSR-probe path can advance RIP past a faulting
        // `rdmsr`/`wrmsr` and let iretq resume normally. Captured BEFORE
        // the 16-byte stack alignment below.
        "lea rdx, [rsp + 128]",
        "mov rbp, rsp",
        "and rsp, -16",
        "call {inner}",
        // Restore GPRs, discard the error code, iretq. The inner returns when
        // it recovered the fault (e.g. armed MSR probe); for an unrecovered
        // Ring-0 GPF the inner panics and never reaches here.
        "mov rsp, rbp",
        "pop rax",  "pop rcx",  "pop rdx",  "pop rbx",
        "pop rbp",  "pop rsi",  "pop rdi",
        "pop r8",   "pop r9",   "pop r10",  "pop r11",
        "pop r12",  "pop r13",  "pop r14",  "pop r15",
        "add rsp, 8",             // drop the CPU-pushed error code
        "iretq",
        inner = sym gpf_inner,
    );
}

#[inline(never)]
extern "C" fn gpf_inner(error_code: u64, saved_cs: u64, saved_rip_slot: *mut u64) {
    // Fault-tolerant MSR access recovery: `msr::rdmsr_safe`/`wrmsr_safe` sets
    // `MSR_ARMED` for the single `rdmsr`/`wrmsr` instruction window. If we
    // landed here with that flag set, the trap is the MSR op against an MSR
    // this CPU doesn't implement (the documented #GP path). Skip the 2-byte
    // instruction by advancing the saved iretq RIP, then return — iretq
    // resumes at the next instruction and the safe wrapper observes
    // `MSR_FAULTED` and returns `None`/`false`. No panic, boot continues.
    //
    // Without this hook the wrappers' `MSR_ARMED` flag is set-but-never-read,
    // and the very first vendor-absent MSR probe (`msr::run_boot_smoketest`'s
    // BOGUS_MSR=0xFFFF_FFFF) takes down the kernel with
    // `GENERAL PROTECTION FAULT in Ring-0 (kernel bug)` even though the
    // wrapper was *meant* to recover. Observed on first AMD Zen 2/3 bare-metal
    // boot.
    if crate::msr::gp_recover_armed() {
        unsafe {
            let rip = core::ptr::read_volatile(saved_rip_slot);
            core::ptr::write_volatile(saved_rip_slot, rip.wrapping_add(2));
        }
        return;
    }
    // Diagnostic: for a userspace #GP, log the faulting RIP (which function
    // GP-faulted → map via nm) + a scan of the user stack for return addresses.
    // iretq frame: RIP@0, CS@1, RFLAGS@2, RSP@3. Bounded to the user half.
    if saved_cs & 3 == 3 {
        unsafe {
            let fault_rip = core::ptr::read_volatile(saved_rip_slot);
            let user_rsp = core::ptr::read_volatile(saved_rip_slot.add(3));
            serial_println!(
                "[#GP] fault_rip={:#x} user_rsp={:#x} err={:#x}",
                fault_rip,
                user_rsp,
                error_code
            );
            if user_rsp != 0 && user_rsp < 0x0000_8000_0000_0000 {
                for i in 0..10u64 {
                    let val = core::ptr::read_volatile((user_rsp + i * 8) as *const u64);
                    serial_println!("[#GP]   [rsp+{:#x}]={:#x}", i * 8, val);
                }
            }
        }
    }
    kill_user_task_or_panic("GENERAL PROTECTION FAULT", error_code, saved_cs)
}

// ── Stack Segment Fault ──────────────────────────────────────────────────

#[unsafe(naked)]
extern "C" fn ssf_handler() {
    // GPRs saved before the error-code read (see gpf_handler) so the saved
    // frame is faithful. Layout after 15 pushes: err@+120, RIP@+128, CS@+136.
    core::arch::naked_asm!(
        "push r15", "push r14", "push r13", "push r12",
        "push r11", "push r10", "push r9",  "push r8",
        "push rdi", "push rsi", "push rbp", "push rbx",
        "push rdx", "push rcx", "push rax",
        "mov rdi, [rsp + 120]",   // error code
        "mov rsi, [rsp + 136]",   // saved CS
        "mov rbp, rsp",
        "and rsp, -16",
        "call {inner}",
        "ud2",
        inner = sym ssf_inner,
    );
}

#[inline(never)]
extern "C" fn ssf_inner(error_code: u64, saved_cs: u64) -> ! {
    kill_user_task_or_panic("STACK SEGMENT FAULT", error_code, saved_cs)
}

// ── Segment Not Present ──────────────────────────────────────────────────

#[unsafe(naked)]
extern "C" fn snp_handler() {
    // GPRs saved before the error-code read (see gpf_handler) so the saved
    // frame is faithful. Layout after 15 pushes: err@+120, RIP@+128, CS@+136.
    core::arch::naked_asm!(
        "push r15", "push r14", "push r13", "push r12",
        "push r11", "push r10", "push r9",  "push r8",
        "push rdi", "push rsi", "push rbp", "push rbx",
        "push rdx", "push rcx", "push rax",
        "mov rdi, [rsp + 120]",   // error code
        "mov rsi, [rsp + 136]",   // saved CS
        "mov rbp, rsp",
        "and rsp, -16",
        "call {inner}",
        "ud2",
        inner = sym snp_inner,
    );
}

#[inline(never)]
extern "C" fn snp_inner(error_code: u64, saved_cs: u64) -> ! {
    kill_user_task_or_panic("SEGMENT NOT PRESENT", error_code, saved_cs)
}

// ── Invalid Opcode ───────────────────────────────────────────────────────

#[unsafe(naked)]
extern "C" fn ud_handler() {
    core::arch::naked_asm!(
        "push r15", "push r14", "push r13", "push r12",
        "push r11", "push r10", "push r9",  "push r8",
        "push rdi", "push rsi", "push rbp", "push rbx",
        "push rdx", "push rcx", "push rax",
        "mov rdi, [rsp + 120]",    // saved RIP
        "mov rsi, [rsp + 128]",    // saved CS
        "mov rbp, rsp",
        "and rsp, -16",
        "call {inner}",
        "ud2",
        inner = sym ud_inner,
    );
}

#[inline(never)]
extern "C" fn ud_inner(rip: u64, saved_cs: u64) -> ! {
    kill_user_task_or_panic("INVALID OPCODE", rip, saved_cs)
}

// ── Page Fault (error code pushed by CPU) ───────────────────────────────

#[unsafe(naked)]
extern "C" fn page_fault_handler() {
    // Save ALL GPRs before reading the error code (same bug class as
    // gpf_handler, fixed 2026-07-08): the old `pop rdi`-first prologue made
    // every RESUMING page fault (the extable/uaccess fixup path) restore
    // rdi = PF ERROR CODE into the interrupted context.
    // Layout after the 15 pushes: err@+120, RIP@+128, CS@+136.
    core::arch::naked_asm!(
        "push r15", "push r14", "push r13", "push r12",
        "push r11", "push r10", "push r9",  "push r8",
        "push rdi", "push rsi", "push rbp", "push rbx",
        "push rdx", "push rcx", "push rax",
        "mov rdi, [rsp + 120]",   // error code (arg1)
        "mov rsi, [rsp + 128]",   // saved RIP  (arg2)
        "mov rdx, [rsp + 136]",   // saved CS   (arg3)
        // arg4 (rcx): pointer to the saved-RIP slot on the iretq frame.
        // The extable fixup path writes through this pointer to rewrite
        // the iretq target — see page_fault_inner. Captured BEFORE the
        // 16-byte stack alignment below so the address is correct
        // relative to the pushed GPRs.
        "lea rcx, [rsp + 128]",
        // arg5 (r8): base of the complete GPR save area.  Retain this for
        // user-fault diagnosis; a userspace C ABI fault otherwise loses the
        // argument registers that identify the bad object pointer.
        "mov r8, rsp",
        "mov rbp, rsp",
        "and rsp, -16",
        "call {inner}",
        "mov rsp, rbp",
        "pop rax",  "pop rcx",  "pop rdx",  "pop rbx",
        "pop rbp",  "pop rsi",  "pop rdi",
        "pop r8",   "pop r9",   "pop r10",  "pop r11",
        "pop r12",  "pop r13",  "pop r14",  "pop r15",
        "add rsp, 8",             // drop the CPU-pushed error code
        "iretq",
        inner = sym page_fault_inner,
    );
}

/// Raw byte to COM1 via direct port I/O. No locks, no fmt — safe to call from a
/// re-entrant fault context where the panic/fmt path is broken.
#[inline(always)]
unsafe fn raw_serial_byte(b: u8) {
    // Wait for THR-empty (LSR bit 5), then write THR (0x3F8).
    loop {
        let lsr: u8;
        core::arch::asm!("in al, dx", out("al") lsr, in("dx") 0x3FDu16, options(nomem, nostack, preserves_flags));
        if lsr & 0x20 != 0 {
            break;
        }
    }
    core::arch::asm!("out dx, al", in("dx") 0x3F8u16, in("al") b, options(nomem, nostack, preserves_flags));
}

unsafe fn raw_serial_str(s: &[u8]) {
    for &b in s {
        raw_serial_byte(b);
    }
}

unsafe fn raw_serial_hex(v: u64) {
    raw_serial_byte(b'0');
    raw_serial_byte(b'x');
    for i in (0..16).rev() {
        let nib = ((v >> (i * 4)) & 0xF) as u8;
        let c = if nib < 10 {
            b'0' + nib
        } else {
            b'a' + nib - 10
        };
        raw_serial_byte(c);
    }
}

#[inline(never)]
extern "C" fn page_fault_inner(
    error_code_raw: u64,
    rip: u64,
    cs: u64,
    saved_rip_slot: *mut u64,
    saved_gprs: *const u64,
) {
    use x86_64::registers::control::Cr2;
    use x86_64::structures::idt::PageFaultErrorCode;
    let fault_addr_raw: u64 = Cr2::read().map(|a| a.as_u64()).unwrap_or(0);
    let error_code = PageFaultErrorCode::from_bits_truncate(error_code_raw);

    let is_user_cs = (cs & 0x3) == 0x3;
    let is_user_addr = (fault_addr_raw >> 47) == 0;
    let is_instr_fetch = error_code.contains(PageFaultErrorCode::INSTRUCTION_FETCH);

    // Ring-0 instruction-fetch at RIP=0 = the kernel called through a NULL
    // function pointer. Falling through to panic!() here is fatal in a specific
    // way: the panic/`core::fmt` path itself performs an indirect call that, on
    // this static-PIE image, can resolve to NULL — so the panic re-faults at
    // RIP=0, re-enters this handler, panics again, and recurses ~1.5 KiB of
    // stack per round until the kernel stack overflows into a #DF → silent
    // triple-fault with ZERO diagnostics. Instead emit a RAW serial marker via
    // direct port I/O (no fmt, no locks, no heap — cannot recurse) and HALT.
    // The caller's return address (pushed by the `call *null`) sits at the top
    // of the faulting RSP; read it defensively for a resolvable site.
    if rip == 0 && !is_user_cs {
        unsafe {
            raw_serial_str(b"\n[FATAL] kernel null-call: jumped to RIP=0 (cs=0x8).");
            let fault_rsp = core::ptr::read_volatile(saved_rip_slot.add(3));
            if fault_rsp >= 0xffff_8000_0000_0000 && (fault_rsp & 0x7) == 0 {
                raw_serial_str(b" caller_ret=");
                raw_serial_hex(core::ptr::read_volatile(fault_rsp as *const u64));
            }
            raw_serial_str(b" -- halting.\n");
        }
        crate::hlt_loop();
    }

    // Extable fault-fixup: if the kernel was inside an `extable::install`'d
    // critical section (today: `copy_from_user` → `copy_user_with_fixup`),
    // rewrite the saved RIP on the iretq frame to the fixup label and
    // return. No locks, no scheduler call — closes the deadlock window
    // where page_fault_inner → has_current_task() → SCHEDULER.lock() would
    // spin forever on a lock the interrupted code holds.
    if !is_user_cs {
        if let Some(fixup_rip) = crate::extable::check(rip) {
            unsafe {
                core::ptr::write_volatile(saved_rip_slot, fixup_rip);
            }
            return;
        }
    }

    // KFENCE guard-page classification (feature = "kfence" only). If the fault
    // landed inside the sampled guard-page pool, classify it (UAF / OOB) and log
    // the recorded alloc/free sites BEFORE the normal handling below. The pool
    // lives in the kernel VA range, so without this hook such a fault would fall
    // through to the generic kernel panic with no diagnosis. `is_kfence_address`
    // is a const-false until the pool is mapped, so this is a no-op early-return
    // for every non-KFENCE fault and for the entire default (feature-off) build.
    #[cfg(feature = "kfence")]
    {
        if crate::hardening::sampler::is_kfence_address(fault_addr_raw) {
            let is_write = error_code.contains(PageFaultErrorCode::CAUSED_BY_WRITE);
            if let Some(kind) = crate::hardening::sampler::classify_fault(fault_addr_raw, is_write)
            {
                serial_println!(
                    "[kfence] GUARD-PAGE FAULT: addr={:#x} rip={:#x} write={} -> {:?}",
                    fault_addr_raw,
                    rip,
                    is_write,
                    kind
                );
            }
            // A real touch of an unmapped guard/freed page is a genuine kernel
            // memory-safety bug. We have now logged the precise classification;
            // fall through to the existing handling, which panics for a ring-0
            // kernel-range fault (the correct loud failure for a real UAF/OOB).
        }
    }

    serial_println!(
        "[PAGE FAULT] addr={:#x} rip={:#x} cs={:#x} err={:?} (user_cs={} user_addr={})",
        fault_addr_raw,
        rip,
        cs,
        error_code,
        is_user_cs,
        is_user_addr
    );

    // The naked entry stub saves registers in reverse push order, with RAX at
    // offset zero.  Preserve the call arguments for ring-3 faults: this makes
    // a bad LinuxKPI C ABI call (for example `drm_exec_init(exec, flags, nr)`)
    // actionable from the serial log without changing the faulting task's
    // execution or touching user memory.
    // Keep this deliberately narrow.  A full register frame is too much work
    // in an exception path (other static PIE programs can share this RIP).
    // These four values cover the SysV C argument boundary plus the Rust
    // structure-return pointer that is null in the AMDGPU failure.
    if is_user_cs && rip == 0x5D127 && !saved_gprs.is_null() {
        unsafe {
            serial_println!(
                "[PAGE FAULT] ABI regs rdi={:#x} rsi={:#x} rdx={:#x} r14={:#x}",
                core::ptr::read_volatile(saved_gprs.add(6)),
                core::ptr::read_volatile(saved_gprs.add(5)),
                core::ptr::read_volatile(saved_gprs.add(2)),
                core::ptr::read_volatile(saved_gprs.add(13)),
            );
        }
    }

    // Kill the offending task (instead of panicking) when the fault is a
    // genuine userspace fault:
    //   * Ring-3 fault (CS RPL=3): a user task touched something bad.
    //   * Ring-0 *data* fault on a user-range address: a syscall handler
    //     dereferenced a bad user pointer (copy_from/to_user is already handled
    //     by the extable above; this is the uninstrumented-deref safety net).
    //
    // A ring-0 *instruction-fetch* fault is NEVER a recoverable user fault — it
    // means the kernel's own RIP jumped into non-executable / bad memory
    // (corrupted function pointer, smashed return address). It must PANIC with
    // the RIP + backtrace, not silently kill a task.
    //
    // The old condition `is_user_cs || is_user_addr` killed on *any* address
    // with bit 47 clear. But this kernel is loaded at virtual base
    // 0x100_0000_0000 (bootloader `virtual_address_offset`), which is < 2^47, so
    // every kernel code/rodata address also satisfies `is_user_addr` — a real
    // kernel control-flow bug was being misclassified as "task T1 faulted" and
    // silently killing the boot thread (a hang with no diagnostics) instead of
    // panicking. MasterChecklist: boot-hang diagnosis.
    if is_user_cs || (is_user_addr && !is_instr_fetch) {
        if crate::scheduler::has_current_task() {
            let tid = crate::scheduler::current_task_id()
                .map(|t| t.raw())
                .unwrap_or(0);
            serial_println!(
                "[PAGE FAULT] Killing faulting task T{} (addr={:#x} rip={:#x})",
                tid,
                fault_addr_raw,
                rip
            );
            // Instruction-fetch fault in userspace = a call/jmp into bad memory
            // (e.g. a stubbed function pointer). The user RSP (iretq frame word 3:
            // RIP@0,CS@1,RFLAGS@2,RSP@3) points at the caller's pushed return
            // address — dump the top of the user stack so the call site can be
            // mapped against `nm -n <daemon>`. Diagnostic only; reads are bounded
            // to the canonical user half and skipped if RSP looks bad.
            // Dump the stack for an instruction-fetch fault, any fault whose RIP
            // is a garbage-low address (a call/ret into a bad pointer, e.g. rip=0x77
            // writing to null), OR any fault whose target is in the low static/stub
            // region (< 2 MiB) — a deref of a bad pointer INTO the binary's static
            // data (e.g. mutex_lock on an uncompiled/stubbed struct), which has a
            // valid RIP + a write and the narrower gate missed. [rsp] holds the
            // caller's return address; map it against `nm -n <daemon>`.
            if is_instr_fetch || rip < 0x1_0000 || fault_addr_raw < 0x0020_0000 {
                // `saved_rip_slot` is the iretq frame on the KERNEL stack (a
                // supervisor mapping) — reading word 3 is SMAP-safe. The user
                // stack it points at is NOT: under CR4.SMAP a raw supervisor
                // read of that user page faults, and in the #PF handler that
                // recurses into #DF. Route the 8-word dump through the uaccess
                // chokepoint (stac/clac + extable fixup) so a bad/unmapped user
                // RSP yields Err, never a nested fault. No allocation (fixed
                // stack buffer) — safe in interrupt context.
                let user_rsp = unsafe { core::ptr::read_volatile(saved_rip_slot.add(3)) };
                if user_rsp != 0 && user_rsp < 0x0000_8000_0000_0000 {
                    serial_println!("[PAGE FAULT] user_rsp={:#x} return-addrs:", user_rsp);
                    let mut words = [0u8; 64]; // 8 × u64
                    if crate::uaccess::copy_from_user_into(user_rsp, &mut words).is_ok() {
                        for i in 0..8usize {
                            let val =
                                u64::from_le_bytes(words[i * 8..i * 8 + 8].try_into().unwrap());
                            serial_println!("[PAGE FAULT]   [rsp+{:#x}]={:#x}", i * 8, val);
                        }
                    } else {
                        serial_println!("[PAGE FAULT]   (user stack unreadable)");
                    }
                }
            }
            crate::scheduler::exit_current_task(0xDEAD_C0DE_u64);
            // exit_current_task does not return.
        } else {
            panic!(
                "PAGE FAULT in user context with no current task: addr={:#x} rip={:#x} err={:?}",
                fault_addr_raw, rip, error_code
            );
        }
    }

    // Ring-0 fault not fixed by the extable (and, above, an instruction-fetch or
    // kernel-range access) → real kernel bug. Panic loudly with full context.
    panic!(
        "KERNEL PAGE FAULT: addr={:#x} rip={:#x} cs={:#x} instr_fetch={} err={:?}",
        fault_addr_raw, rip, cs, is_instr_fetch, error_code
    );
}

// ── Machine Check (no error code, divergent) ────────────────────────────

#[unsafe(naked)]
extern "C" fn machine_check_handler() {
    // #MC pushes no error code. CPU interrupt frame after our 15 pushes:
    //   RIP @ rsp+120, CS @ rsp+128. We pass the saved CS to the inner fn so
    //   it can decide kernel-vs-userspace context.
    core::arch::naked_asm!(
        "push r15", "push r14", "push r13", "push r12",
        "push r11", "push r10", "push r9",  "push r8",
        "push rdi", "push rsi", "push rbp", "push rbx",
        "push rdx", "push rcx", "push rax",
        "mov rdi, [rsp + 128]",   // saved CS (arg1)
        "mov rbp, rsp",
        "and rsp, -16",
        "call {inner}",
        // machine_check_inner may return (recovered correctable error): restore
        // GPRs and iretq back to the interrupted context.
        "mov rsp, rbp",
        "pop rax",  "pop rcx",  "pop rdx",  "pop rbx",
        "pop rbp",  "pop rsi",  "pop rdi",
        "pop r8",   "pop r9",   "pop r10",  "pop r11",
        "pop r12",  "pop r13",  "pop r14",  "pop r15",
        "iretq",
        inner = sym machine_check_inner,
    );
}

#[inline(never)]
extern "C" fn machine_check_inner(saved_cs: u64) {
    let mcg = crate::mce::read_mcg_status();
    let banks = crate::mce::bank_count();
    let from_user = (saved_cs & 0x3) == 0x3;
    // Walks MCA banks: clears correctable errors and returns; for uncorrectable
    // errors it either kills the faulting user task (does not return) or panics
    // (kernel context). If it returns here, the error was correctable/recovered.
    crate::mce::handle_machine_check(mcg, banks, from_user);
}

// ── Timer (no error code) ───────────────────────────────────────────────
// Already uses the naked→inner pattern — kept as-is.

#[unsafe(naked)]
pub extern "C" fn timer_interrupt_handler() {
    core::arch::naked_asm!(
        "push r15",
        "push r14",
        "push r13",
        "push r12",
        "push r11",
        "push r10",
        "push r9",
        "push r8",
        "push rdi",
        "push rsi",
        "push rbp",
        "push rbx",
        "push rdx",
        "push rcx",
        "push rax",
        "mov rbp, rsp",
        "and rsp, -16",
        "mov rdi, rbp",
        "call timer_handler_inner",
        "mov rsp, rbp",
        "pop rax",
        "pop rcx",
        "pop rdx",
        "pop rbx",
        "pop rbp",
        "pop rsi",
        "pop rdi",
        "pop r8",
        "pop r9",
        "pop r10",
        "pop r11",
        "pop r12",
        "pop r13",
        "pop r14",
        "pop r15",
        "iretq",
    );
}

// ── Keyboard (no error code) ────────────────────────────────────────────

#[unsafe(naked)]
extern "C" fn keyboard_interrupt_handler() {
    core::arch::naked_asm!(
        "push r15", "push r14", "push r13", "push r12",
        "push r11", "push r10", "push r9",  "push r8",
        "push rdi", "push rsi", "push rbp", "push rbx",
        "push rdx", "push rcx", "push rax",
        "mov rbp, rsp",
        "and rsp, -16",
        "call {inner}",
        "mov rsp, rbp",
        "pop rax",  "pop rcx",  "pop rdx",  "pop rbx",
        "pop rbp",  "pop rsi",  "pop rdi",
        "pop r8",   "pop r9",   "pop r10",  "pop r11",
        "pop r12",  "pop r13",  "pop r14",  "pop r15",
        "iretq",
        inner = sym keyboard_inner,
    );
}

#[inline(never)]
extern "C" fn keyboard_inner() {
    use x86_64::instructions::port::Port;

    let mut port = Port::new(0x60);
    let scancode: u8 = unsafe { port.read() };

    // Send scancode to Channel 1 (Keyboard driver)
    let msg = crate::ipc::Message {
        msg_type: 1, // Keyboard Event
        arg1: scancode as u64,
        arg2: 0,
        arg3: 0,
    };

    // Telemetry: input event + TSC stamp for /proc/raeen/perf (latency proxy).
    crate::perf::record_input_event();

    // Use try_lock() to avoid deadlock if IPC is already held.
    // It's better to drop a keystroke than deadlock the system.
    if let Some(mut ipc) = crate::ipc::IPC.try_lock() {
        let _ = ipc.send(crate::ipc::KEYBOARD_CHANNEL, msg);
    }

    crate::arch::interrupt_controller::eoi();

    // Wake any userspace driver blocked on SYS_IRQ_WAIT(this vector).
    crate::scheduler::unblock_irq_waiters(InterruptIndex::Keyboard.as_u8());

    // Wake any task blocked on SYS_RECV(handle whose chan_id == KEYBOARD_CHANNEL).
    // This is the missing half of input routing — without it a userspace event
    // loop calling SYS_RECV stays parked forever even when scancodes arrive.
    crate::scheduler::unblock_receivers(crate::ipc::KEYBOARD_CHANNEL);

    // Deliver scancode to the focused app's per-task key buffer so
    // SYS_READ_KEY works for userspace processes.
    if let Some(tid) = crate::compositor::focused_task_id() {
        crate::scheduler::with_task_by_id(tid, |task| {
            task.push_key(scancode);
        });
    }

    crate::shell_runner::handle_key(scancode);
}

// ── Spurious (no error code) ────────────────────────────────────────────

#[unsafe(naked)]
extern "C" fn spurious_interrupt_handler() {
    core::arch::naked_asm!(
        // A spurious interrupt is a hardware artifact. Just iretq.
        "iretq",
    );
}

// ── APIC Error (no error code) ──────────────────────────────────────────

#[unsafe(naked)]
extern "C" fn apic_error_handler() {
    core::arch::naked_asm!(
        "push r15", "push r14", "push r13", "push r12",
        "push r11", "push r10", "push r9",  "push r8",
        "push rdi", "push rsi", "push rbp", "push rbx",
        "push rdx", "push rcx", "push rax",
        "mov rbp, rsp",
        "and rsp, -16",
        "call {inner}",
        "mov rsp, rbp",
        "pop rax",  "pop rcx",  "pop rdx",  "pop rbx",
        "pop rbp",  "pop rsi",  "pop rdi",
        "pop r8",   "pop r9",   "pop r10",  "pop r11",
        "pop r12",  "pop r13",  "pop r14",  "pop r15",
        "iretq",
        inner = sym apic_error_inner,
    );
}

#[inline(never)]
extern "C" fn apic_error_inner() {
    // BUG-22: read the Error Status Register (the helper does the mandated
    // write-0-then-read) so the error is actually cleared and identified.
    // Without this the LVT can keep re-firing on the same latched condition,
    // flooding the log. EOI so the LAPIC can deliver further interrupts.
    let esr = crate::apic::read_error_status();
    serial_println!("[EXCEPTION] APIC Error: ESR={:#010x}", esr);
    crate::arch::interrupt_controller::eoi();
}

// ── System Control Interrupt (SCI) ──────────────────────────────────────

#[unsafe(naked)]
extern "C" fn sci_interrupt_handler() {
    core::arch::naked_asm!(
        "push r15", "push r14", "push r13", "push r12",
        "push r11", "push r10", "push r9",  "push r8",
        "push rdi", "push rsi", "push rbp", "push rbx",
        "push rdx", "push rcx", "push rax",
        "mov rbp, rsp",
        "and rsp, -16",
        "call {inner}",
        "mov rsp, rbp",
        "pop rax",  "pop rcx",  "pop rdx",  "pop rbx",
        "pop rbp",  "pop rsi",  "pop rdi",
        "pop r8",   "pop r9",   "pop r10",  "pop r11",
        "pop r12",  "pop r13",  "pop r14",  "pop r15",
        "iretq",
        inner = sym sci_inner,
    );
}

#[inline(never)]
extern "C" fn sci_inner() {
    crate::gpe::on_sci_interrupt();
    crate::arch::interrupt_controller::eoi();
}

// ── PS/2 Mouse ──────────────────────────────────────────────────────────

/// PS/2 mouse state machine: collects 3-byte packets (byte 0 = flags,
/// byte 1 = delta-X, byte 2 = delta-Y) and pushes a complete event into
/// the mouse IPC channel (Channel 2).
static MOUSE_CYCLE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
static MOUSE_BYTE: [core::sync::atomic::AtomicU8; 3] = [
    core::sync::atomic::AtomicU8::new(0),
    core::sync::atomic::AtomicU8::new(0),
    core::sync::atomic::AtomicU8::new(0),
];

#[unsafe(naked)]
extern "C" fn mouse_interrupt_handler() {
    core::arch::naked_asm!(
        "push r15", "push r14", "push r13", "push r12",
        "push r11", "push r10", "push r9",  "push r8",
        "push rdi", "push rsi", "push rbp", "push rbx",
        "push rdx", "push rcx", "push rax",
        "mov rbp, rsp",
        "and rsp, -16",
        "call {inner}",
        "mov rsp, rbp",
        "pop rax",  "pop rcx",  "pop rdx",  "pop rbx",
        "pop rbp",  "pop rsi",  "pop rdi",
        "pop r8",   "pop r9",   "pop r10",  "pop r11",
        "pop r12",  "pop r13",  "pop r14",  "pop r15",
        "iretq",
        inner = sym mouse_inner,
    );
}

#[inline(never)]
extern "C" fn mouse_inner() {
    use core::sync::atomic::Ordering::Relaxed;
    use x86_64::instructions::port::Port;

    let mut port: Port<u8> = Port::new(0x60);
    let byte: u8 = unsafe { port.read() };

    let cycle = MOUSE_CYCLE.load(Relaxed);
    // Guard against out-of-bounds index if cycle is corrupted.
    if cycle as usize >= MOUSE_BYTE.len() {
        MOUSE_CYCLE.store(0, Relaxed);
        crate::arch::interrupt_controller::eoi();
        return;
    }
    MOUSE_BYTE[cycle as usize].store(byte, Relaxed);

    if cycle == 2 {
        let flags = MOUSE_BYTE[0].load(Relaxed);
        let dx = MOUSE_BYTE[1].load(Relaxed) as i8;
        let dy = MOUSE_BYTE[2].load(Relaxed) as i8;
        let buttons = flags & 0x07;

        let msg = crate::ipc::Message {
            msg_type: 2, // Mouse Event
            arg1: dx as i64 as u64,
            arg2: dy as i64 as u64,
            arg3: buttons as u64,
        };
        // Use try_lock() to avoid deadlock if IPC is already held.
        if let Some(mut ipc) = crate::ipc::IPC.try_lock() {
            let _ = ipc.send(crate::ipc::MOUSE_CHANNEL, msg);
        }

        MOUSE_CYCLE.store(0, Relaxed);

        crate::compositor::move_cursor(dx as i32, dy as i32);

        if let Some(tid) = crate::compositor::focused_task_id() {
            crate::scheduler::with_task_by_id(tid, |task| {
                task.push_mouse(dx as i16, -(dy as i16), buttons);
            });
        }

        crate::shell_runner::handle_mouse(dx as i32, -(dy as i32), buttons);

        crate::scheduler::unblock_receivers(2);
    } else {
        MOUSE_CYCLE.store(cycle + 1, Relaxed);
    }

    crate::arch::interrupt_controller::eoi();
}

/// Initialize the PS/2 mouse controller. Must be called after IOAPIC setup.
pub fn init_ps2_mouse() {
    use x86_64::instructions::port::Port;

    let mut cmd_port: Port<u8> = Port::new(0x64);
    let mut data_port: Port<u8> = Port::new(0x60);

    let wait_input = || {
        for _ in 0..100_000 {
            let status: u8 = unsafe { Port::<u8>::new(0x64).read() };
            if (status & 0x02) == 0 {
                return;
            }
            core::hint::spin_loop();
        }
    };
    let wait_output = || {
        for _ in 0..100_000 {
            let status: u8 = unsafe { Port::<u8>::new(0x64).read() };
            if (status & 0x01) != 0 {
                return;
            }
            core::hint::spin_loop();
        }
    };

    unsafe {
        // Enable the auxiliary (mouse) PS/2 port.
        wait_input();
        cmd_port.write(0xA8);

        // Get the controller config byte.
        wait_input();
        cmd_port.write(0x20);
        wait_output();
        let mut config = data_port.read();

        // Enable IRQ12 (bit 1) and clear mouse-disable (bit 5).
        config |= 0x02;
        config &= !0x20;

        wait_input();
        cmd_port.write(0x60);
        wait_input();
        data_port.write(config);

        // Send "reset" to the mouse device.
        wait_input();
        cmd_port.write(0xD4);
        wait_input();
        data_port.write(0xFF);
        wait_output();
        let _ = data_port.read(); // ACK
                                  // The mouse sends two more bytes (0xAA, 0x00) on self-test pass.
        for _ in 0..2 {
            wait_output();
            let _ = data_port.read();
        }

        // Enable data reporting.
        wait_input();
        cmd_port.write(0xD4);
        wait_input();
        data_port.write(0xF4);
        wait_output();
        let _ = data_port.read(); // ACK
    }

    // Route IRQ12 (GSI 12) -> our Mouse vector through the IOAPIC.
    crate::apic::route_mouse_irq();
    serial_println!(
        "[ OK ] PS/2 Mouse initialized (IRQ12 -> vector {})",
        InterruptIndex::Mouse.as_u8()
    );
}

// ── VirtIO Block (no error code) ────────────────────────────────────────

#[unsafe(naked)]
extern "C" fn virtio_blk_interrupt_handler() {
    core::arch::naked_asm!(
        "push r15", "push r14", "push r13", "push r12",
        "push r11", "push r10", "push r9",  "push r8",
        "push rdi", "push rsi", "push rbp", "push rbx",
        "push rdx", "push rcx", "push rax",
        "mov rbp, rsp",
        "and rsp, -16",
        "call {inner}",
        "mov rsp, rbp",
        "pop rax",  "pop rcx",  "pop rdx",  "pop rbx",
        "pop rbp",  "pop rsi",  "pop rdi",
        "pop r8",   "pop r9",   "pop r10",  "pop r11",
        "pop r12",  "pop r13",  "pop r14",  "pop r15",
        "iretq",
        inner = sym virtio_blk_inner,
    );
}

#[inline(never)]
extern "C" fn virtio_blk_inner() {
    if let Some(blk) = crate::virtio::VIRTIO_BLK.get() {
        use x86_64::instructions::port::PortReadOnly;

        // Read ISR status to clear the interrupt (virtio spec requires this for PCI)
        let mut isr_port: PortReadOnly<u8> = PortReadOnly::new(blk.port_base + 0x13);
        let status = unsafe { isr_port.read() };

        if (status & 1) != 0 {
            // try_lock, never lock(): mainline read_block/write_block hold this
            // queue lock (e.g. during submit_request, with IRQs enabled). A
            // blocking lock here would spin forever on the very lock the
            // interrupted code holds — a single-core re-entrant deadlock that
            // froze the boot at random points. If the queue is busy, the
            // mainline poll loop processes the used ring itself, so skipping is
            // safe.
            if let Some(mut q) = blk.queue.try_lock() {
                q.process_used_ring();
            }
        }
    }

    crate::arch::interrupt_controller::eoi();
}

// ── VirtIO Net (no error code) ──────────────────────────────────────────

#[unsafe(naked)]
extern "C" fn virtio_net_interrupt_handler() {
    core::arch::naked_asm!(
        "push r15", "push r14", "push r13", "push r12",
        "push r11", "push r10", "push r9",  "push r8",
        "push rdi", "push rsi", "push rbp", "push rbx",
        "push rdx", "push rcx", "push rax",
        "mov rbp, rsp",
        "and rsp, -16",
        "call {inner}",
        "mov rsp, rbp",
        "pop rax",  "pop rcx",  "pop rdx",  "pop rbx",
        "pop rbp",  "pop rsi",  "pop rdi",
        "pop r8",   "pop r9",   "pop r10",  "pop r11",
        "pop r12",  "pop r13",  "pop r14",  "pop r15",
        "iretq",
        inner = sym virtio_net_inner,
    );
}

#[inline(never)]
extern "C" fn virtio_net_inner() {
    if let Some(net) = crate::virtio_net::VIRTIO_NET.get() {
        net.irq_top_half();
    }
    crate::arch::interrupt_controller::eoi();
}
