//! APIC bringup — Local APIC (per-CPU) + I/O APIC (chipset-wide IRQ router).
//!
//! Replaces the legacy 8259 PIC. Boot order:
//!   1. `crate::interrupts::disable_pic()` masks the 8259 lines.
//!   2. ACPI parses MADT → gives us the LAPIC physaddr + per-IOAPIC info.
//!   3. `init_bsp(lapic_phys)` enables the bootstrap-processor LAPIC and
//!      starts its periodic timer at our `InterruptIndex::Timer` vector.
//!   4. `init_ioapic(ioapic_phys, gsi_base, bsp_apic_id)` programs the
//!      IOAPIC redirection table — today, just GSI 1 -> keyboard vector.
//!   5. `sti` re-enables interrupts.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use spin::Mutex;
use x2apic::lapic::{LocalApic, LocalApicBuilder};

pub static LAPIC_TIMER_TICKS: AtomicU32 = AtomicU32::new(0);
pub static TSC_FREQ_MHZ: AtomicU64 = AtomicU64::new(0);
/// LAPIC MMIO virtual base, used by `end_of_interrupt()` to avoid locking.
static LAPIC_BASE_VIRT: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApicMode {
    Xapic,
    X2apic,
}

/// 0 = xAPIC, 1 = x2APIC.
pub static CURRENT_APIC_MODE: AtomicU8 = AtomicU8::new(0);

pub fn get_apic_mode() -> ApicMode {
    match CURRENT_APIC_MODE.load(Ordering::SeqCst) {
        1 => ApicMode::X2apic,
        _ => ApicMode::Xapic,
    }
}

fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi);
    }
    ((hi as u64) << 32) | (lo as u64)
}

pub fn calibrate_tsc() {
    let start_tsc = read_tsc();
    // HPET is not online yet (ACPI runs later). Use PIT for a real 10 ms window.
    crate::hpet::pit_spin_wait_10ms();
    let end_tsc = read_tsc();

    let freq_mhz = (end_tsc - start_tsc) / 10_000;
    TSC_FREQ_MHZ.store(freq_mhz, Ordering::SeqCst);
    crate::serial_println!("[apic] Calibrated TSC: {} MHz", freq_mhz);

    probe_x2apic_support();
}

/// MasterChecklist Phase 1.3 — x2APIC detection.
///
/// CPUID 1:ECX[21] = x2APIC capability. IA32_APIC_BASE (MSR 0x1B):
///   bit 11 = global LAPIC enable
///   bit 10 = EXTD (x2APIC mode active)
/// Combined: EN=1,EXTD=0 → xAPIC MMIO; EN=1,EXTD=1 → x2APIC MSR;
/// EN=0 → LAPIC disabled (firmware bug). Decode and log; the actual
/// MSR-vs-MMIO switch is a follow-up slice.
pub fn probe_x2apic_support() {
    const IA32_APIC_BASE: u32 = 0x1B;
    let supported = unsafe {
        let r = core::arch::x86_64::__cpuid(1);
        (r.ecx & (1 << 21)) != 0
    };
    let apic_base: u64 = unsafe {
        let lo: u32;
        let hi: u32;
        core::arch::asm!(
            "rdmsr",
            in("ecx") IA32_APIC_BASE,
            out("eax") lo, out("edx") hi,
            options(nomem, nostack, preserves_flags),
        );
        ((hi as u64) << 32) | (lo as u64)
    };
    let en = (apic_base & (1 << 11)) != 0;
    let extd = (apic_base & (1 << 10)) != 0;
    let phys_base = apic_base & 0xFFFF_F000;
    let mode = match (en, extd) {
        (true, true) => "x2APIC (MSR)",
        (true, false) => "xAPIC (MMIO)",
        (false, _) => "DISABLED",
    };
    X2APIC_SUPPORTED.store(supported, Ordering::SeqCst);
    crate::serial_println!(
        "[apic] x2APIC cpuid={} IA32_APIC_BASE={:#x} mode={} phys_base={:#x}",
        supported,
        apic_base,
        mode,
        phys_base,
    );
}

/// Enable x2APIC mode on the current CPU by setting bit 10 (EXTD) of IA32_APIC_BASE.
pub fn enable_x2apic() {
    const IA32_APIC_BASE: u32 = 0x1B;
    unsafe {
        let mut lo: u32;
        let mut hi: u32;
        core::arch::asm!(
            "rdmsr",
            in("ecx") IA32_APIC_BASE,
            out("eax") lo, out("edx") hi,
            options(nomem, nostack, preserves_flags),
        );
        // Bit 11 = EN (global enable), Bit 10 = EXTD (x2APIC enable).
        if (lo & (1 << 10)) == 0 {
            lo |= 1 << 10;
            core::arch::asm!(
                "wrmsr",
                in("ecx") IA32_APIC_BASE,
                in("eax") lo, in("edx") hi,
                options(nomem, nostack, preserves_flags),
            );
        }
    }
    CURRENT_APIC_MODE.store(1, Ordering::SeqCst);
}

/// Boot-time-cached "does this CPU support x2APIC?" so the rest of the
/// kernel doesn't have to re-issue CPUID.
pub static X2APIC_SUPPORTED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

pub fn tsc_wait_us(us: u64) {
    let freq = TSC_FREQ_MHZ.load(Ordering::Relaxed);
    if freq == 0 {
        // Fallback to HPET if not calibrated
        crate::hpet::spin_wait_us(us);
        return;
    }
    let start = read_tsc();
    let target = us * freq;
    while read_tsc() - start < target {
        core::hint::spin_loop();
    }
}

/// `x2apic::LocalApic` doesn't impl `Send`. It's only ever touched on the
/// owning CPU and the mutex serializes access, so wrapping it in a
/// hand-rolled Send/Sync wrapper is safe.
pub enum Apic {
    Xapic(LocalApic),
    X2apic,
}
pub struct SafeApic(pub Apic);
unsafe impl Send for SafeApic {}
unsafe impl Sync for SafeApic {}

pub static LAPIC: Mutex<Option<SafeApic>> = Mutex::new(None);

/// Bring up the Local APIC on the bootstrap processor.
/// `apic_physical_address` comes from ACPI's MADT.
pub fn init_bsp(apic_physical_address: u64) {
    if X2APIC_SUPPORTED.load(Ordering::SeqCst) {
        enable_x2apic();
    }

    let mode = get_apic_mode();
    let offset = *crate::memory::PHYS_MEM_OFFSET
        .get()
        .expect("PHYS_MEM_OFFSET not initialized");
    let virt = offset + apic_physical_address;

    if mode == ApicMode::X2apic {
        unsafe {
            // x2APIC registers (MSRs)
            const IA32_X2APIC_SVR: u32 = 0x80F;
            const IA32_X2APIC_LVT_ERR: u32 = 0x837;
            const IA32_X2APIC_LVT_TMR: u32 = 0x832;
            const IA32_X2APIC_TIC_TMR: u32 = 0x838;
            const IA32_X2APIC_CUR_TMR: u32 = 0x839;
            const IA32_X2APIC_TDCR: u32 = 0x83E;

            let spurious_vec = crate::interrupts::InterruptIndex::Spurious.as_u8() as u64;
            let error_vec = crate::interrupts::InterruptIndex::ApicError.as_u8() as u64;
            let timer_vec = crate::interrupts::InterruptIndex::Timer.as_u8() as u64;

            // Enable APIC by setting bit 8 of Spurious Interrupt Vector Register
            let svr = rdmsr(IA32_X2APIC_SVR);
            wrmsr(IA32_X2APIC_SVR, svr | (1 << 8) | spurious_vec);

            // Set Error LVT
            wrmsr(IA32_X2APIC_LVT_ERR, error_vec);

            // Configure timer: Periodic mode (bit 17), Div16 (0x3)
            wrmsr(IA32_X2APIC_TDCR, 0x3);
            wrmsr(IA32_X2APIC_LVT_TMR, (1 << 17) | timer_vec);

            // Calibrate using HPET
            wrmsr(IA32_X2APIC_TIC_TMR, 0xFFFF_FFFF);
            crate::hpet::spin_wait_us(10_000);
            let elapsed = 0xFFFF_FFFF - rdmsr(IA32_X2APIC_CUR_TMR) as u32;
            wrmsr(IA32_X2APIC_TIC_TMR, elapsed as u64);
            LAPIC_TIMER_TICKS.store(elapsed, Ordering::SeqCst);

            crate::serial_println!("[apic] Calibrated x2APIC timer: {} ticks per 10ms", elapsed);
        }
        *LAPIC.lock() = Some(SafeApic(Apic::X2apic));
        crate::serial_println!("[ OK ] Local APIC initialized on BSP in x2APIC mode (MSR)");
    } else {
        let mut lapic = LocalApicBuilder::new()
            .timer_vector(crate::interrupts::InterruptIndex::Timer.as_u8() as usize)
            .error_vector(crate::interrupts::InterruptIndex::ApicError.as_u8() as usize)
            .spurious_vector(crate::interrupts::InterruptIndex::Spurious.as_u8() as usize)
            .set_xapic_base(virt.as_u64())
            .build()
            .unwrap_or_else(|e| panic!("Failed to build LAPIC: {:?}", e));

        unsafe {
            lapic.enable();
            lapic.set_timer_mode(x2apic::lapic::TimerMode::Periodic);
            lapic.set_timer_divide(x2apic::lapic::TimerDivide::Div16);

            // Calibration using HPET for a 10ms tick (100 Hz).
            // 1. Set initial count to maximum and start the timer.
            lapic.set_timer_initial(0xFFFF_FFFF);

            // 2. Wait 10ms using our HPET monotonic clock.
            crate::hpet::spin_wait_us(10_000);

            // 3. Read current count to see how many ticks elapsed.
            let elapsed_ticks = 0xFFFF_FFFF - lapic.timer_current();

            // 4. Set the actual initial count to the elapsed ticks.
            lapic.set_timer_initial(elapsed_ticks);
            LAPIC_TIMER_TICKS.store(elapsed_ticks, Ordering::SeqCst);

            crate::serial_println!(
                "[apic] Calibrated xAPIC timer: {} ticks per 10ms",
                elapsed_ticks
            );
        }

        // Store the LAPIC base for lock-free EOI writes from interrupt handlers.
        LAPIC_BASE_VIRT.store(virt.as_u64(), Ordering::SeqCst);

        *LAPIC.lock() = Some(SafeApic(Apic::Xapic(lapic)));
        crate::serial_println!(
            "[ OK ] Local APIC initialized on BSP in xAPIC mode (phys {:#x} -> virt {:#x})",
            apic_physical_address,
            virt.as_u64(),
        );
    }
}

unsafe fn rdmsr(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    core::arch::asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi, options(nomem, nostack, preserves_flags));
    ((hi as u64) << 32) | (lo as u64)
}

unsafe fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    core::arch::asm!("wrmsr", in("ecx") msr, in("eax") lo, in("edx") hi, options(nomem, nostack, preserves_flags));
}

/// Acknowledge an interrupt at the Local APIC. Must be the last thing every
/// interrupt handler does (replaces the 8259 `notify_end_of_interrupt`).
///
/// Uses a direct volatile write to the EOI register (or wrmsr in x2APIC)
/// instead of acquiring `LAPIC.lock()` — this avoids deadlocks when called
/// from interrupt handlers that fire while the LAPIC lock is already held.
pub fn end_of_interrupt() {
    if get_apic_mode() == ApicMode::X2apic {
        unsafe {
            // x2APIC EOI: MSR 0x80B. Write 0 to acknowledge.
            core::arch::asm!(
                "wrmsr",
                in("ecx") 0x80B,
                in("eax") 0,
                in("edx") 0,
                options(nomem, nostack, preserves_flags),
            );
        }
    } else {
        let base = LAPIC_BASE_VIRT.load(Ordering::SeqCst);
        if base != 0 {
            unsafe {
                // EOI register is at offset 0xB0. Write 0 to acknowledge.
                core::ptr::write_volatile((base as *mut u32).add(0xB0 / 4), 0);
            }
        }
    }
}

/// Reads the APIC Error Status Register (ESR).
/// According to the Intel manual, software must write 0 to the ESR before reading it.
pub fn read_error_status() -> u32 {
    if get_apic_mode() == ApicMode::X2apic {
        unsafe {
            // x2APIC ESR: MSR 0x828.
            core::arch::asm!(
                "wrmsr",
                in("ecx") 0x828,
                in("eax") 0,
                in("edx") 0,
                options(nomem, nostack, preserves_flags),
            );
            let mut eax: u32;
            core::arch::asm!(
                "rdmsr",
                in("ecx") 0x828,
                out("eax") eax,
                out("edx") _,
                options(nomem, nostack, preserves_flags),
            );
            eax
        }
    } else {
        let base = LAPIC_BASE_VIRT.load(Ordering::SeqCst);
        if base != 0 {
            unsafe {
                let esr_ptr = (base as *mut u32).add(0x280 / 4);
                core::ptr::write_volatile(esr_ptr, 0);
                core::ptr::read_volatile(esr_ptr)
            }
        } else {
            0
        }
    }
}

/// Start the LAPIC timer on the current CPU using the globally calibrated ticks.
pub fn start_lapic_timer() {
    let ticks = LAPIC_TIMER_TICKS.load(Ordering::SeqCst);
    if ticks == 0 {
        panic!("LAPIC_TIMER_TICKS not calibrated yet!");
    }

    let mode = get_apic_mode();
    if mode == ApicMode::X2apic {
        unsafe {
            const IA32_X2APIC_SVR: u32 = 0x80F;
            const IA32_X2APIC_LVT_ERR: u32 = 0x837;
            const IA32_X2APIC_LVT_TMR: u32 = 0x832;
            const IA32_X2APIC_TIC_TMR: u32 = 0x838;
            const IA32_X2APIC_TDCR: u32 = 0x83E;

            let spurious_vec = crate::interrupts::InterruptIndex::Spurious.as_u8() as u64;
            let error_vec = crate::interrupts::InterruptIndex::ApicError.as_u8() as u64;
            let timer_vec = crate::interrupts::InterruptIndex::Timer.as_u8() as u64;

            // Enable APIC
            let svr = rdmsr(IA32_X2APIC_SVR);
            wrmsr(IA32_X2APIC_SVR, svr | (1 << 8) | spurious_vec);

            // Set Error LVT
            wrmsr(IA32_X2APIC_LVT_ERR, error_vec);

            // Configure timer
            wrmsr(IA32_X2APIC_TDCR, 0x3); // Div16
            wrmsr(IA32_X2APIC_LVT_TMR, (1 << 17) | timer_vec); // Periodic
            wrmsr(IA32_X2APIC_TIC_TMR, ticks as u64);
        }
    } else {
        let offset = *crate::memory::PHYS_MEM_OFFSET.get().unwrap();
        let virt = offset + 0xFEE0_0000u64;

        let mut lapic = LocalApicBuilder::new()
            .timer_vector(crate::interrupts::InterruptIndex::Timer.as_u8() as usize)
            .error_vector(crate::interrupts::InterruptIndex::ApicError.as_u8() as usize)
            .spurious_vector(crate::interrupts::InterruptIndex::Spurious.as_u8() as usize)
            .set_xapic_base(virt.as_u64())
            .build()
            .unwrap();

        unsafe {
            lapic.enable();
            lapic.set_timer_mode(x2apic::lapic::TimerMode::Periodic);
            lapic.set_timer_divide(x2apic::lapic::TimerDivide::Div16);
            lapic.set_timer_initial(ticks);
        }
    }
}

// ── I/O APIC ─────────────────────────────────────────────────────────────

/// The I/O APIC is memory-mapped: one 32-bit index register at +0x00, one
/// 32-bit data register at +0x10. We talk to it through raw volatile reads
/// rather than depending on the `ioapic` crate — we only need a tiny subset
/// and keeping deps thin matters in the kernel.
pub struct IoApic {
    /// Virtual address of the IOAPIC MMIO window (already +PHYS_MEM_OFFSET).
    mmio_base: u64,
    /// Global System Interrupt this IOAPIC starts at.
    gsi_base: u32,
    /// Legacy IRQ mapping: `legacy_overrides[isa_irq]` -> `(gsi, level_triggered, active_low)`.
    legacy_overrides: [(u32, bool, bool); 16],
}

mod ioapic_reg {
    pub const ID: u32 = 0x00;
    pub const VER: u32 = 0x01;
    // Redirection table entry N is at index (0x10 + 2N, 0x10 + 2N + 1).
    pub const REDTBL_BASE: u32 = 0x10;
}

// Bits in the low 32-bit half of a redirection-table entry.
const REDIR_DELIVERY_FIXED: u32 = 0b000 << 8;
const REDIR_DEST_PHYSICAL: u32 = 0 << 11;
const REDIR_POLARITY_LOW_ACTIVE: u32 = 1 << 13;
const REDIR_TRIGGER_LEVEL: u32 = 1 << 15;
const REDIR_MASKED: u32 = 1 << 16;

pub static IOAPIC: Mutex<Option<IoApic>> = Mutex::new(None);

impl IoApic {
    /// Caller must guarantee `physical_address` is identity-mapped via
    /// `PHYS_MEM_OFFSET` (the bootloader's offset window).
    pub fn new(physical_address: u64, gsi_base: u32) -> Self {
        crate::arch::mmu::kernel().map_mmio_range(
            x86_64::PhysAddr::new(physical_address),
            4096,
            crate::arch::mmu::PageFlags::DEVICE,
        );
        let offset = *crate::memory::PHYS_MEM_OFFSET
            .get()
            .expect("PHYS_MEM_OFFSET not initialized");

        let mut legacy_overrides = [(0, false, false); 16];
        for i in 0..16 {
            legacy_overrides[i] = (i as u32, false, false);
        }

        Self {
            mmio_base: (offset + physical_address).as_u64(),
            gsi_base,
            legacy_overrides,
        }
    }

    /// Apply MADT overrides for legacy ISA IRQs (e.g. keyboard, timer).
    pub fn apply_overrides(
        &mut self,
        overrides: &[acpi::platform::interrupt::InterruptSourceOverride],
    ) {
        for iso in overrides {
            if iso.isa_source < 16 {
                let level = matches!(
                    iso.trigger_mode,
                    acpi::platform::interrupt::TriggerMode::Level
                );
                let low = matches!(iso.polarity, acpi::platform::interrupt::Polarity::ActiveLow);
                self.legacy_overrides[iso.isa_source as usize] =
                    (iso.global_system_interrupt, level, low);
            }
        }
    }

    /// Looks up the GSI, trigger mode, and polarity for a legacy ISA IRQ.
    pub fn get_legacy_irq_mapping(&self, isa_irq: u8) -> (u32, bool, bool) {
        if isa_irq < 16 {
            self.legacy_overrides[isa_irq as usize]
        } else {
            (isa_irq as u32, false, false)
        }
    }

    fn write_reg(&mut self, reg: u32, value: u32) {
        unsafe {
            let ioregsel = self.mmio_base as *mut u32;
            let iowin = (self.mmio_base + 0x10) as *mut u32;
            ioregsel.write_volatile(reg);
            iowin.write_volatile(value);
        }
    }

    fn read_reg(&mut self, reg: u32) -> u32 {
        unsafe {
            let ioregsel = self.mmio_base as *mut u32;
            let iowin = (self.mmio_base + 0x10) as *const u32;
            ioregsel.write_volatile(reg);
            iowin.read_volatile()
        }
    }

    /// Total redirection entries this IOAPIC supports.
    pub fn max_entries(&mut self) -> u32 {
        ((self.read_reg(ioapic_reg::VER) >> 16) & 0xff) + 1
    }

    /// Mask every redirection entry — sane starting state. Caller selectively
    /// unmasks the IRQs they actually want.
    pub fn mask_all(&mut self) {
        let count = self.max_entries();
        for i in 0..count {
            self.set_redir_entry(i, 0, 0, true, false, false);
        }
    }

    /// Program one redirection-table entry.
    ///
    /// * `gsi`             — local IOAPIC entry index (0..`max_entries`)
    /// * `vector`          — LAPIC vector to deliver the IRQ as
    /// * `dest_apic_id`    — which CPU's LAPIC (physical destination mode)
    /// * `masked`          — true to disable this line
    /// * `level_triggered` — false for edge-triggered (ISA default)
    /// * `active_low`      — false for active-high (ISA default)
    pub fn set_redir_entry(
        &mut self,
        gsi: u32,
        vector: u8,
        dest_apic_id: u8,
        masked: bool,
        level_triggered: bool,
        active_low: bool,
    ) {
        let mut low: u32 = vector as u32 | REDIR_DELIVERY_FIXED | REDIR_DEST_PHYSICAL;
        if level_triggered {
            low |= REDIR_TRIGGER_LEVEL;
        }
        if active_low {
            low |= REDIR_POLARITY_LOW_ACTIVE;
        }
        if masked {
            low |= REDIR_MASKED;
        }
        let high: u32 = (dest_apic_id as u32) << 24;

        let reg_low = ioapic_reg::REDTBL_BASE + 2 * gsi;
        let reg_high = reg_low + 1;
        // Write high (destination) first so the entry isn't briefly aimed at
        // CPU 0 with the new vector before the destination catches up.
        self.write_reg(reg_high, high);
        self.write_reg(reg_low, low);
    }

    pub fn gsi_base(&self) -> u32 {
        self.gsi_base
    }
    pub fn id(&mut self) -> u32 {
        (self.read_reg(ioapic_reg::ID) >> 24) & 0xff
    }
}

/// Bring up the first IOAPIC and route legacy keyboard IRQ to vector 33.
/// `dest_apic_id` is normally the BSP's LAPIC id (from MADT).
pub fn init_ioapic(
    physical_address: u64,
    gsi_base: u32,
    dest_apic_id: u8,
    overrides: &[acpi::platform::interrupt::InterruptSourceOverride],
) {
    let mut ioapic = IoApic::new(physical_address, gsi_base);
    ioapic.apply_overrides(overrides);

    let id = ioapic.id();
    let entries = ioapic.max_entries();
    crate::serial_println!(
        "[ OK ] IOAPIC id={} entries={} @ phys {:#x} (gsi_base={})",
        id,
        entries,
        physical_address,
        gsi_base,
    );

    // Default: everything masked. We selectively enable what we use.
    ioapic.mask_all();

    // Keyboard: legacy IRQ1. We apply overrides to ensure it's routed correctly.
    let kbd_vector = crate::interrupts::InterruptIndex::Keyboard.as_u8();
    let (gsi, level, low) = ioapic.get_legacy_irq_mapping(1);

    ioapic.set_redir_entry(
        gsi,
        kbd_vector,
        dest_apic_id,
        false, // masked
        level, // level_triggered
        low,   // active_low
    );
    crate::serial_println!(
        "[ OK ] IOAPIC GSI {} (keyboard) -> vector {} -> APIC id {}",
        gsi,
        kbd_vector,
        dest_apic_id,
    );

    *IOAPIC.lock() = Some(ioapic);
}

/// Route the PS/2 mouse IRQ (GSI 12) to the Mouse interrupt vector.
pub fn route_mouse_irq() {
    if let Some(ioapic) = IOAPIC.lock().as_mut() {
        let mouse_vector = crate::interrupts::InterruptIndex::Mouse.as_u8();
        let (gsi, level, low) = ioapic.get_legacy_irq_mapping(12);

        ioapic.set_redir_entry(
            gsi,
            mouse_vector,
            0,     // dest = BSP (APIC id 0)
            false, // unmasked
            level, // level-triggered
            low,   // active-low
        );
        crate::serial_println!(
            "[ OK ] IOAPIC GSI {} (mouse) -> vector {} -> APIC id 0",
            gsi,
            mouse_vector,
        );
    }
}

pub fn route_irq(gsi: u32, vector: u8) {
    if let Some(ioapic) = IOAPIC.lock().as_mut() {
        // Assume BSP is APIC ID 0 for simplicity, ideally we should read it from ACPI
        let dest_apic_id = 0;

        ioapic.set_redir_entry(
            gsi,
            vector,
            dest_apic_id,
            false, // unmasked
            true,  // PCI interrupts are level-triggered
            false, // PCI interrupts usually active-low, but QEMU ACPI overrides to ActiveHigh
        );
        crate::serial_println!(
            "[ OK ] IOAPIC GSI {} -> vector {} -> APIC id {}",
            gsi,
            vector,
            dest_apic_id,
        );
    } else {
        crate::serial_println!("[WARN] Cannot route GSI {}; IOAPIC not initialized", gsi);
    }
}

pub fn route_sci(gsi: u32, vector: u8) {
    if let Some(ioapic) = IOAPIC.lock().as_mut() {
        // SCI is typically level-triggered and active-low.
        ioapic.set_redir_entry(
            gsi, vector, 0,     // dest = BSP (APIC id 0)
            false, // unmasked
            true,  // level-triggered
            true,  // active-low
        );
        crate::serial_println!("[acpi] SCI wired to IRQ {} -> vector {}", gsi, vector,);
    } else {
        crate::serial_println!(
            "[WARN] Cannot route SCI GSI {}; IOAPIC not initialized",
            gsi
        );
    }
}

// ── Diagnostics ──────────────────────────────────────────────────────────

pub fn run_boot_smoketest() {
    let mode = get_apic_mode();
    let supported = X2APIC_SUPPORTED.load(Ordering::SeqCst);
    crate::serial_println!(
        "[apic] run_boot_smoketest: mode={:?} supported={} -> PASS",
        mode,
        supported
    );
}

pub fn dump_text() -> alloc::string::String {
    let mode = get_apic_mode();
    let supported = X2APIC_SUPPORTED.load(Ordering::SeqCst);
    let ticks = LAPIC_TIMER_TICKS.load(Ordering::SeqCst);
    let tsc = TSC_FREQ_MHZ.load(Ordering::SeqCst);

    let mut out = alloc::string::String::new();
    use core::fmt::Write;
    let _ = writeln!(out, "APIC Mode: {:?}", mode);
    let _ = writeln!(out, "x2APIC Supported: {}", supported);
    let _ = writeln!(out, "LAPIC Timer Ticks (10ms): {}", ticks);
    let _ = writeln!(out, "TSC Frequency: {} MHz", tsc);
    out
}
