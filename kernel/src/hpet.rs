use spin::Mutex;

pub struct Hpet {
    mmio_base: u64,
    /// The period of the main counter in femtoseconds (10^-15 seconds).
    /// Typically around 10,000,000 fs (10 ns) to 100,000,000 fs (100 ns).
    clock_period_fs: u32,
}

const REG_CONFIG: u64 = 0x010;
const REG_COUNTER: u64 = 0x0F0;

pub static HPET: Mutex<Option<Hpet>> = Mutex::new(None);

/// True once ACPI HPET init succeeded.
pub fn is_initialized() -> bool {
    HPET.lock().is_some()
}

pub fn init(physical_address: u64) {
    let offset = *crate::memory::PHYS_MEM_OFFSET
        .get()
        .expect("PHYS_MEM_OFFSET not initialized");
    let mmio_base = offset.as_u64() + physical_address;

    // Read capabilities to get the clock period
    let capabilities = unsafe { core::ptr::read_volatile(mmio_base as *const u64) };
    let clock_period_fs = (capabilities >> 32) as u32;

    crate::serial_println!(
        "[hpet] Found at phys {:#x}, period: {} fs",
        physical_address,
        clock_period_fs
    );

    // Enable the main counter
    let config_ptr = (mmio_base + REG_CONFIG) as *mut u64;
    unsafe {
        let mut config = core::ptr::read_volatile(config_ptr);
        config |= 1; // Set bit 0 to enable counting
        core::ptr::write_volatile(config_ptr, config);
    }

    let boot_counter = unsafe { core::ptr::read_volatile((mmio_base + REG_COUNTER) as *const u64) };

    *HPET.lock() = Some(Hpet {
        mmio_base,
        clock_period_fs,
    });

    crate::serial_println!("[ OK ] HPET initialized and monotonic clock started");
    crate::rtc::on_hpet_ready(clock_period_fs, boot_counter);
}

impl Hpet {
    pub fn read_counter(&self) -> u64 {
        let counter_ptr = (self.mmio_base + REG_COUNTER) as *const u64;
        unsafe { core::ptr::read_volatile(counter_ptr) }
    }

    pub fn period_fs(&self) -> u32 {
        self.clock_period_fs
    }
}

/// Re-enable the main counter after S3 resume. The platform reset that
/// accompanies the wake clears GENERAL_CONFIG.ENABLE_CNF, freezing the
/// counter — every `spin_wait_us` then waits forever on a stopped clock
/// (observed 2026-07-02 as the post-resume AP re-online wedge inside
/// `start_one_ap`'s INIT settle wait). Idempotent; no-op when HPET was never
/// found.
pub fn reenable_after_resume() {
    if let Some(hpet) = HPET.lock().as_ref() {
        let config_ptr = (hpet.mmio_base + REG_CONFIG) as *mut u64;
        unsafe {
            let mut config = core::ptr::read_volatile(config_ptr);
            config |= 1; // ENABLE_CNF
            core::ptr::write_volatile(config_ptr, config);
        }
    }
}

/// Read the raw HPET main counter. Returns None if HPET is not initialized.
pub fn read_counter() -> Option<u64> {
    if let Some(hpet) = HPET.lock().as_ref() {
        let counter_ptr = (hpet.mmio_base + REG_COUNTER) as *const u64;
        let value = unsafe { core::ptr::read_volatile(counter_ptr) };
        Some(value)
    } else {
        None
    }
}

const PIT_CHANNEL2: u16 = 0x42;
const PIT_CMD: u16 = 0x43;
const PIT_GATE: u16 = 0x61;
const PIT_HZ: u64 = 1_193_182;

/// ~10 ms one-shot wait on PIT channel 2. Used for early TSC calibration
/// before ACPI has brought HPET online (MasterChecklist Phase 1.6).
pub fn pit_spin_wait_10ms() {
    pit_spin_wait_us(10_000);
}

/// Spin-wait using the legacy 8254 PIT when HPET is hidden or not yet init.
pub fn pit_spin_wait_us(microseconds: u64) {
    if microseconds == 0 {
        return;
    }
    // PIT resolution is ~838 ns at max rate; clamp to at least one tick.
    let ticks = (microseconds * PIT_HZ / 1_000_000)
        .max(1)
        .min(u64::from(u16::MAX));
    unsafe {
        let mut cmd = x86_64::instructions::port::Port::<u8>::new(PIT_CMD);
        let mut ch2 = x86_64::instructions::port::Port::<u8>::new(PIT_CHANNEL2);
        let mut gate = x86_64::instructions::port::Port::<u8>::new(PIT_GATE);

        cmd.write(0xB0); // ch2, lobyte/hibyte, mode 0, binary
        let count = ticks as u16;
        ch2.write((count & 0xFF) as u8);
        ch2.write((count >> 8) as u8);

        let saved_gate = gate.read();
        gate.write((saved_gate & 0xFD) | 0x01); // enable ch2 gate

        loop {
            if gate.read() & 0x20 != 0 {
                break;
            }
            core::hint::spin_loop();
        }
        gate.write(saved_gate);
    }
}

/// Spin polling `cond()` until it returns true OR `timeout_us` of
/// wall-clock time has elapsed. Returns `true` on success, `false` on
/// timeout. Replaces the boot-path anti-pattern of
/// `for _ in 0..N { if cond { return Ok }; spin_loop }` which couples
/// the timeout to CPU speed: on QEMU TCG 500_000 iterations is ~500ms,
/// on a real 4.5 GHz core it's ~110µs — so the timeout is meaningless
/// in both directions. Use this when waiting for a hardware register
/// bit to flip (USB controller ready, MSR settling, completion events,
/// etc.) so the wait scales with reality, not iteration count.
///
/// Checks the wall clock every 256 polls of `cond()` to keep PCIe /
/// MMIO traffic low — most callers' `cond` reads an MMIO register, and
/// the HPET counter read is similarly priced.
pub fn spin_until_us<F: FnMut() -> bool>(timeout_us: u64, mut cond: F) -> bool {
    // Snapshot the start time once; cheap repeated `now_us` reads are
    // what we're trying to avoid in the inner loop.
    let mut iter: u32 = 0;
    let start_check = || {
        let hpet_guard = HPET.lock();
        if let Some(hpet) = hpet_guard.as_ref() {
            let counter_ptr = (hpet.mmio_base + REG_COUNTER) as *const u64;
            let start = unsafe { core::ptr::read_volatile(counter_ptr) };
            let wait_ticks = (timeout_us * 1_000_000_000) / (hpet.clock_period_fs as u64);
            let mmio = hpet.mmio_base;
            let period = hpet.clock_period_fs as u64;
            drop(hpet_guard);
            return Some((start, wait_ticks, mmio, period));
        }
        None
    };
    let hpet_state = start_check();

    loop {
        if cond() {
            return true;
        }
        iter = iter.wrapping_add(1);
        // Coarse-grained deadline check — once every 256 polls.
        if iter & 0xFF == 0 {
            if let Some((start, wait_ticks, mmio, _period)) = hpet_state {
                let counter_ptr = (mmio + REG_COUNTER) as *const u64;
                let current = unsafe { core::ptr::read_volatile(counter_ptr) };
                if current.wrapping_sub(start) >= wait_ticks {
                    return false;
                }
            } else {
                // HPET absent → fall back to TSC-based deadline.
                let freq_mhz =
                    crate::apic::TSC_FREQ_MHZ.load(core::sync::atomic::Ordering::Relaxed);
                if freq_mhz > 0 {
                    static START_TSC: core::sync::atomic::AtomicU64 =
                        core::sync::atomic::AtomicU64::new(0);
                    let now = unsafe { core::arch::x86_64::_rdtsc() };
                    let start = START_TSC.load(core::sync::atomic::Ordering::Relaxed);
                    if start == 0 {
                        START_TSC.store(now, core::sync::atomic::Ordering::Relaxed);
                    } else {
                        let target_cycles = timeout_us.saturating_mul(freq_mhz);
                        if now.saturating_sub(start) >= target_cycles {
                            START_TSC.store(0, core::sync::atomic::Ordering::Relaxed);
                            return false;
                        }
                    }
                }
                // No timer at all → bounded iteration fallback so we
                // can't hang forever. Generous: ~50ms on a slow QEMU.
                if iter > 5_000_000 {
                    return false;
                }
            }
        }
        core::hint::spin_loop();
    }
}

/// Spin-wait for the specified number of microseconds using the HPET.
pub fn spin_wait_us(microseconds: u64) {
    let hpet_guard = HPET.lock();
    if let Some(hpet) = hpet_guard.as_ref() {
        // Convert microseconds to femtoseconds: 1 us = 10^9 fs
        let wait_fs = microseconds * 1_000_000_000;
        let wait_ticks = wait_fs / (hpet.clock_period_fs as u64);

        let counter_ptr = (hpet.mmio_base + REG_COUNTER) as *const u64;
        let start = unsafe { core::ptr::read_volatile(counter_ptr) };

        drop(hpet_guard); // Don't hold the lock while spinning

        loop {
            let current = unsafe { core::ptr::read_volatile(counter_ptr) };
            if current.wrapping_sub(start) >= wait_ticks {
                break;
            }
            core::hint::spin_loop();
        }
        return;
    }
    drop(hpet_guard);

    // HPET absent or not yet parsed — TSC if calibrated, else PIT (not a busy-spin guess).
    let freq_mhz = crate::apic::TSC_FREQ_MHZ.load(core::sync::atomic::Ordering::Relaxed);
    if freq_mhz > 0 {
        let target = microseconds.saturating_mul(freq_mhz);
        let start = unsafe { core::arch::x86_64::_rdtsc() };
        while unsafe { core::arch::x86_64::_rdtsc() }.saturating_sub(start) < target {
            core::hint::spin_loop();
        }
    } else {
        pit_spin_wait_us(microseconds);
    }
}

/// Boot smoketest: exercise the PIT fallback path (MasterChecklist Phase 1.6).
pub fn run_boot_smoketest() {
    let t0 = unsafe { core::arch::x86_64::_rdtsc() };
    pit_spin_wait_us(500);
    let t1 = unsafe { core::arch::x86_64::_rdtsc() };
    let pit_ok = t1 > t0;
    crate::serial_println!(
        "[hpet] smoketest: pit_fallback_us=500 tsc_delta={} -> {}",
        t1.saturating_sub(t0),
        if pit_ok { "PASS" } else { "FAIL" }
    );
}

/// Called from ACPI when the HPET table is missing. MasterChecklist Phase 1.6.
pub fn note_absent() {
    crate::serial_println!(
        "[hpet] absent — spin_wait_us falls back to PIT/TSC; wall clock uses RTC anchor"
    );
    crate::rtc::on_hpet_absent();
}

/// Read the HPET counter converted to milliseconds.
/// Returns None if HPET is not initialized.
/// Conversion: ticks * period_fs / 1_000_000_000_000
pub fn read_millis() -> Option<i64> {
    let hpet_guard = HPET.lock();
    if let Some(hpet) = hpet_guard.as_ref() {
        let counter_ptr = (hpet.mmio_base + REG_COUNTER) as *const u64;
        let ticks = unsafe { core::ptr::read_volatile(counter_ptr) };
        let period_fs = hpet.clock_period_fs as u64;
        // ticks * period_fs gives femtoseconds; divide by 10^12 to get milliseconds
        let ms = (ticks / 1_000_000) * period_fs / 1_000_000;
        Some(ms as i64)
    } else {
        // HPET absent or hidden on some firmware — fall back to RTC/TSC wall clock.
        let epoch_s = crate::rtc::now_epoch_secs();
        Some((epoch_s as i64) * 1000)
    }
}
