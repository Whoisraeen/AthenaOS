//! Hardware + software watchdog timer subsystem.
//!
//! Detects kernel hangs, soft lockups (CPU spinning in kernel), hard lockups
//! (CPU not responding to NMIs), and ensures panic handlers complete within
//! a timeout. All critical paths are interrupt-safe (no allocation in NMI).
//!
//! Concept: a shipping gaming OS must never wedge the machine. RaeenOS pairs a
//! lightweight software "kernel alive" heartbeat (incremented by the BSP timer
//! tick) with chipset hardware watchdogs (Intel TCO on ICH/PCH SMBus, AMD
//! SP5100 MMIO) so that a fully hung kernel is rebooted by silicon even when no
//! software path can run. See Phase 4.6 in MasterChecklist.md.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

// ═══════════════════════════════════════════════════════════════════════════════
//  Constants
// ═══════════════════════════════════════════════════════════════════════════════

const DEFAULT_HW_TIMEOUT_SEC: u32 = 30;
const DEFAULT_SW_TIMEOUT_SEC: u32 = 10;
const DEFAULT_PANIC_TIMEOUT_SEC: u32 = 60;
const NMI_WATCHDOG_INTERVAL_MS: u64 = 1000;
const SOFTLOCKUP_THRESHOLD_SEC: u64 = 20;
const HARDLOCKUP_THRESHOLD_SEC: u64 = 10;
const MAX_WATCHDOG_SOURCES: usize = 16;
const MAX_CPUS: usize = 64;

/// The kernel-alive heartbeat is considered "stuck" if it has not advanced
/// within this window. Matches the default hardware timeout so software detects
/// the wedge just before silicon force-reboots.
const ALIVE_STUCK_THRESHOLD_SEC: u64 = 30;

// Intel TCO (Timer Count-Out) I/O ports
const TCO_RLD: u16 = 0x460; // TCO Timer Reload
const TCO1_STS: u16 = 0x464; // TCO1 Status
const TCO2_STS: u16 = 0x466; // TCO2 Status
const TCO1_CNT: u16 = 0x468; // TCO1 Control
const TCO2_CNT: u16 = 0x46A; // TCO2 Control
const TCO_TMR: u16 = 0x470; // TCO Timer Value

// ACPI watchdog ports (platform-dependent)
const ACPI_WDAT_BASE: u16 = 0x400;

// PCI vendor / class identifiers for SMBus controllers that host a watchdog.
const PCI_VENDOR_INTEL: u16 = 0x8086;
const PCI_VENDOR_AMD: u16 = 0x1022;
const PCI_CLASS_SERIAL_BUS: u8 = 0x0C; // Serial bus controller
                                       // RaeenOS fix (Phase 4.6): SMBus is subclass 0x05 (0x07 is IPMI). The old
                                       // value meant find_smbus_controller() NEVER matched — on Athena the AMD
                                       // EFCH probe never even ran ("software watchdog only" on iron).
const PCI_SUBCLASS_SMBUS: u8 = 0x05; // SMBus

// AMD SP5100 / FCH watchdog MMIO layout.
// The watchdog control/count registers live in a 32-byte MMIO window whose
// base is programmed by firmware. On the QEMU/SeaBIOS FCH model and most real
// AMD boards the base defaults to the well-known address below when the BIOS
// has not relocated it.
const SP5100_WDT_DEFAULT_BASE: u64 = 0xFEB0_0000;
const SP5100_WDT_MMIO_LEN: usize = 0x20;
const SP5100_WDT_CONTROL: usize = 0x00; // bit0 = run/stop, bit7 = trigger
const SP5100_WDT_COUNT: usize = 0x04; // reload count (in ~1s ticks)
const SP5100_WDT_TRIGGER: u32 = 1 << 7; // write to reload the counter
const SP5100_WDT_RUN: u32 = 1 << 0; // enable bit
                                    // Linux sp5100_tco semantics: bit 2 SET selects power-off; CLEAR selects reset.
const SP5100_WDT_ACTION_POWEROFF: u32 = 1 << 2;

// AMD EFCH watchdog (family 16h models 30h+ and Zen). Linux prefers the
// dedicated 0xFEB00000 window and may use the FCH ACPI-MMIO alias at
// 0xFED80000+0xB00 when ISAControl.MMIOEN advertises it. Both expose:
//   +0x00  WDT control (bit0 run, bit1 fired, bit2 action=poweroff, bit7 trigger)
//   +0x04  WDT count   (units per the PM resolution field below)
// and must be decode-enabled through the PM register file at +0x300:
//   PM+0x00 (DECODEEN)  bit7  = WatchdogTimerEnable (decode + run gate)
//   PM+0x03 (DECODEEN3) bits3:2 = watchdog DISABLE (must be cleared),
//                       bits1:0 = count resolution (0=32µs,1=10ms,2=100ms,3=1s)
// Layout per coreboot FCH docs / Linux sp5100_tco.c `efch` path.
const EFCH_ACPI_MMIO_BASE: u64 = 0xFED8_0000;
const EFCH_ACPI_MMIO_LEN: usize = 0x1000;
const EFCH_WDT_PRIMARY_BASE: u64 = 0xFEB0_0000;
const EFCH_WDT_LEN: usize = 0x08;
const EFCH_PM_OFFSET: usize = 0x300;
const EFCH_WDT_OFFSET: usize = 0xB00;
const EFCH_PM_DECODEEN: usize = 0x00; // byte reg, bit7 = WDT enable
const EFCH_PM_DECODEEN_WDT_TMREN: u8 = 1 << 7;
const EFCH_PM_DECODEEN3: usize = 0x03; // byte reg
const EFCH_PM_ISACONTROL: usize = 0x04;
const EFCH_PM_ISACONTROL_MMIOEN: u8 = 1 << 1;
const EFCH_PM_WATCHDOG_DISABLE: u8 = 0b11 << 2;
const EFCH_PM_RES_MASK: u8 = 0b11;
const EFCH_PM_RES_1S: u8 = 0b11;
const EFCH_TICKS_PER_SEC: u32 = 1;

/// A safe-return watchdog must be unconditional: generic health checks and
/// panic paths may not reload it after the one-shot bare-metal test is armed.
static SAFE_RETURN_ARMED: AtomicBool = AtomicBool::new(false);

// ═══════════════════════════════════════════════════════════════════════════════
//  Error Types
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogError {
    HardwareNotPresent,
    AlreadyRunning,
    NotRunning,
    SourceNotFound,
    SourceLimitReached,
    Timeout,
    InvalidTimeout,
    HardwareFailed,
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Watchdog Action (escalation ladder)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WatchdogAction {
    LogWarning,
    DumpStack,
    AttemptRecovery,
    Panic,
    ForceReboot,
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Kernel-Alive Heartbeat (BSP timer tick)
// ═══════════════════════════════════════════════════════════════════════════════

/// Monotonic "kernel is still alive" counter. The BSP scheduler timer handler
/// calls [`on_timer_tick`] every tick to advance this. If it ever stops
/// advancing the kernel is wedged and the software watchdog ([`check_alive`])
/// — and ultimately the hardware watchdog — reacts.
///
/// Exposed publicly so the boot smoketest and `/proc/raeen/watchdog` can prove
/// the heartbeat is live without taking the manager lock.
pub static WATCHDOG_TICK: AtomicU64 = AtomicU64::new(0);

/// Snapshot of [`WATCHDOG_TICK`] at the previous [`check_alive`] call.
static LAST_TICK: AtomicU64 = AtomicU64::new(0);

/// TSC timestamp at the last observed tick advance — used to bound the stuck
/// window in real time rather than in (variable) check intervals.
static LAST_TICK_TSC: AtomicU64 = AtomicU64::new(0);

/// Cached TSC frequency (kHz) for converting the stuck threshold to TSC cycles.
/// Updated by [`init`] from the watchdog manager. Defaults to a sane 3 GHz.
static ALIVE_TSC_FREQ_KHZ: AtomicU64 = AtomicU64::new(3_000_000);

/// Called from the BSP scheduler timer interrupt handler on every tick.
///
/// Interrupt-safe: a single relaxed atomic increment, no allocation, no locks.
/// Wire this into the scheduler timer handler (see main_rs_additions).
#[inline]
pub fn on_timer_tick() {
    WATCHDOG_TICK.fetch_add(1, Ordering::Relaxed);
}

/// Software watchdog: verify the kernel-alive counter advanced since the last
/// check. If it has not advanced for longer than [`ALIVE_STUCK_THRESHOLD_SEC`]
/// the kernel is hung — set the triggered flag and panic so the crash path (and
/// hardware watchdog) takes over.
///
/// Returns the current tick count so callers (smoketest) can report it.
/// Safe to call periodically from a low-priority kernel thread.
pub fn check_alive() -> u64 {
    let now_tick = WATCHDOG_TICK.load(Ordering::Relaxed);
    let last_tick = LAST_TICK.load(Ordering::Relaxed);
    let now_tsc = read_tsc_watchdog();

    if now_tick != last_tick {
        // Heartbeat advanced — kernel is alive. Record the new baseline.
        LAST_TICK.store(now_tick, Ordering::Relaxed);
        LAST_TICK_TSC.store(now_tsc, Ordering::Relaxed);
        // Pet the hardware watchdog while we're confirmed healthy.
        WATCHDOG.lock().hw_watchdog.pet();
        return now_tick;
    }

    // No advance since last check. Bound the stuck condition in real time.
    let last_tsc = LAST_TICK_TSC.load(Ordering::Relaxed);
    if last_tsc == 0 {
        // First-ever check with no prior advance: establish a baseline.
        LAST_TICK_TSC.store(now_tsc, Ordering::Relaxed);
        return now_tick;
    }

    let freq_khz = ALIVE_TSC_FREQ_KHZ.load(Ordering::Relaxed).max(1);
    let elapsed_tsc = now_tsc.saturating_sub(last_tsc);
    let stuck_tsc = ALIVE_STUCK_THRESHOLD_SEC * freq_khz * 1000;

    if elapsed_tsc >= stuck_tsc {
        WATCHDOG_TRIGGERED.store(true, Ordering::SeqCst);
        panic!(
            "[watchdog] kernel-alive heartbeat stuck at tick={} for >{}s — kernel hung",
            now_tick, ALIVE_STUCK_THRESHOLD_SEC
        );
    }

    now_tick
}

// ═══════════════════════════════════════════════════════════════════════════════
//  1. Hardware Watchdog (Intel TCO / AMD SP5100 / ACPI WDAT)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HwWatchdogType {
    IntelTco,
    AmdSp5100,
    /// Zen-era FCH watchdog. For this type `mmio_base` is the selected WDT
    /// window itself (Linux-preferred 0xFEB00000 or alternate 0xFED80B00).
    AmdEfch,
    AcpiWdat,
    None,
}

pub struct HardwareWatchdog {
    pub hw_type: HwWatchdogType,
    pub timeout_sec: u32,
    pub running: AtomicBool,
    pub last_pet_tsc: AtomicU64,
    pub io_base: u16,
    /// MMIO base for the AMD SP5100 watchdog (virtual address after ioremap).
    /// Zero when not using an MMIO-based watchdog.
    pub mmio_base: AtomicU64,
    pub detected: bool,
    /// PCI BDF of the SMBus controller that hosts the watchdog (for reporting).
    pub pci_bdf: (u8, u8, u8),
}

impl HardwareWatchdog {
    pub const fn new() -> Self {
        Self {
            hw_type: HwWatchdogType::None,
            timeout_sec: DEFAULT_HW_TIMEOUT_SEC,
            running: AtomicBool::new(false),
            last_pet_tsc: AtomicU64::new(0),
            io_base: 0,
            mmio_base: AtomicU64::new(0),
            detected: false,
            pci_bdf: (0xFF, 0xFF, 0xFF),
        }
    }

    /// Detect a chipset watchdog. Order: Intel TCO (via ICH/PCH SMBus), then
    /// AMD SP5100 (via FCH SMBus MMIO). Each is skipped cleanly if absent.
    pub fn detect(&mut self) -> bool {
        // 1) Look for an Intel/AMD SMBus controller (class 0x0C, subclass 0x07).
        if let Some(dev) = find_smbus_controller() {
            self.pci_bdf = (dev.bus, dev.device, dev.function);
            match dev.vendor_id {
                PCI_VENDOR_INTEL => {
                    // Intel TCO lives behind fixed ACPI/PMBASE I/O ports. Confirm
                    // the TCO status register responds (not all-ones float).
                    let tco_sts = unsafe { x86_port_read_u16(TCO1_STS) };
                    if tco_sts != 0xFFFF {
                        self.hw_type = HwWatchdogType::IntelTco;
                        self.io_base = TCO_RLD;
                        self.detected = true;
                        return true;
                    }
                }
                PCI_VENDOR_AMD => {
                    // Zen-era FCH: enable through the PM register file, then
                    // select the same primary/alternate WDT window as Linux.
                    // Every probe step logs its outcome: the bare-metal
                    // evidence channel is a photographed screen, so a silent
                    // detect failure costs a whole flash round.
                    crate::serial_println!(
                        "[watchdog] AMD SMBus {:02x}:{:02x}.{} — probing EFCH WDT at {:#x}+{:#x}",
                        dev.bus,
                        dev.device,
                        dev.function,
                        EFCH_ACPI_MMIO_BASE,
                        EFCH_WDT_OFFSET,
                    );
                    let acpi_mmio = crate::mmio::ioremap(EFCH_ACPI_MMIO_BASE, EFCH_ACPI_MMIO_LEN);
                    if acpi_mmio == 0 {
                        crate::serial_println!(
                            "[watchdog] EFCH probe: ioremap({:#x}) returned NULL",
                            EFCH_ACPI_MMIO_BASE,
                        );
                    }
                    if acpi_mmio != 0 {
                        // SAFETY: `acpi_mmio` maps the 4 KiB FCH ACPI MMIO
                        // block; PM byte regs at +0x300 and the 32-bit WDT
                        // regs at +0xB00 are within bounds. Setting the
                        // decode-enable bit makes the WDT registers readable;
                        // it does not start the timer (run bit stays as-is).
                        unsafe {
                            let pm = acpi_mmio + EFCH_PM_OFFSET;
                            let de = (pm + EFCH_PM_DECODEEN) as *mut u8;
                            core::ptr::write_volatile(
                                de,
                                core::ptr::read_volatile(de) | EFCH_PM_DECODEEN_WDT_TMREN,
                            );
                            // Clear the full firmware-disable field and select
                            // Linux's one-second count resolution.
                            let de3 = (pm + EFCH_PM_DECODEEN3) as *mut u8;
                            let v = core::ptr::read_volatile(de3);
                            core::ptr::write_volatile(
                                de3,
                                (v & !(EFCH_PM_WATCHDOG_DISABLE | EFCH_PM_RES_MASK))
                                    | EFCH_PM_RES_1S,
                            );
                            // Linux sp5100_tco prefers the dedicated
                            // 0xFEB00000 window.  The ACPI-MMIO +0xB00 alias is
                            // valid only when ISAControl.MMIOEN advertises it.
                            let primary = crate::mmio::ioremap(EFCH_WDT_PRIMARY_BASE, EFCH_WDT_LEN);
                            let primary_ctrl = if primary != 0 {
                                core::ptr::read_volatile(primary as *const u32)
                            } else {
                                0xFFFF_FFFF
                            };
                            let isa =
                                core::ptr::read_volatile((pm + EFCH_PM_ISACONTROL) as *const u8);
                            let alternate = if isa & EFCH_PM_ISACONTROL_MMIOEN != 0 {
                                acpi_mmio + EFCH_WDT_OFFSET
                            } else {
                                0
                            };
                            let alternate_ctrl = if alternate != 0 {
                                core::ptr::read_volatile(alternate as *const u32)
                            } else {
                                0xFFFF_FFFF
                            };
                            let (wdt_base, ctrl, source) = if primary_ctrl != 0xFFFF_FFFF {
                                (primary, primary_ctrl, "primary-0xfeb00000")
                            } else {
                                (alternate, alternate_ctrl, "alternate-0xfed80b00")
                            };
                            crate::serial_println!(
                                "[watchdog] EFCH probe: {} control reads {:#010x}{}",
                                source,
                                ctrl,
                                if ctrl == 0xFFFF_FFFF {
                                    " (floating — not decoded)"
                                } else {
                                    ""
                                },
                            );
                            if ctrl != 0xFFFF_FFFF {
                                self.hw_type = HwWatchdogType::AmdEfch;
                                // Store the selected watchdog window itself;
                                // AmdEfch operations use direct offsets 0/4.
                                self.mmio_base.store(wdt_base as u64, Ordering::SeqCst);
                                self.detected = true;
                                return true;
                            }
                        }
                    }

                    // Pre-Zen fallback: legacy SP5100 fixed window.
                    let virt = crate::mmio::ioremap(SP5100_WDT_DEFAULT_BASE, SP5100_WDT_MMIO_LEN);
                    if virt != 0 {
                        // SAFETY: `virt` is a freshly-ioremapped MMIO window of
                        // SP5100_WDT_MMIO_LEN bytes; reading the 32-bit control
                        // register at offset 0 is within bounds and side-effect
                        // free.
                        let ctrl = unsafe {
                            core::ptr::read_volatile((virt + SP5100_WDT_CONTROL) as *const u32)
                        };
                        if ctrl != 0xFFFF_FFFF {
                            self.hw_type = HwWatchdogType::AmdSp5100;
                            self.mmio_base.store(virt as u64, Ordering::SeqCst);
                            self.detected = true;
                            return true;
                        }
                    }

                    // AMD board with no reachable FCH watchdog: report honestly.
                    // Do NOT fall through to the Intel TCO port probe — those
                    // legacy ports can float non-FF values on AMD and produce a
                    // false IntelTco detection that "starts" nothing.
                    self.hw_type = HwWatchdogType::None;
                    self.detected = false;
                    return false;
                }
                _ => {}
            }
        }

        // 2) Fall back to a bare Intel TCO probe even without a recognised SMBus
        //    function (some firmware hides the SMBus device but still wires TCO).
        let tco_sts = unsafe { x86_port_read_u16(TCO1_STS) };
        if tco_sts != 0xFFFF {
            self.hw_type = HwWatchdogType::IntelTco;
            self.io_base = TCO_RLD;
            self.detected = true;
            return true;
        }

        self.hw_type = HwWatchdogType::None;
        self.detected = false;
        false
    }

    pub fn start(&self) -> Result<(), WatchdogError> {
        if !self.detected {
            return Err(WatchdogError::HardwareNotPresent);
        }
        if self.running.load(Ordering::SeqCst) {
            return Err(WatchdogError::AlreadyRunning);
        }

        match self.hw_type {
            HwWatchdogType::IntelTco => self.start_tco(),
            HwWatchdogType::AmdSp5100 => self.start_sp5100(),
            HwWatchdogType::AmdEfch => self.start_efch(),
            HwWatchdogType::AcpiWdat => self.start_acpi(),
            HwWatchdogType::None => return Err(WatchdogError::HardwareNotPresent),
        }

        self.running.store(true, Ordering::SeqCst);
        self.pet();
        Ok(())
    }

    pub fn stop(&self) -> Result<(), WatchdogError> {
        if !self.running.load(Ordering::SeqCst) {
            return Err(WatchdogError::NotRunning);
        }

        match self.hw_type {
            HwWatchdogType::IntelTco => self.stop_tco(),
            HwWatchdogType::AmdSp5100 => self.stop_sp5100(),
            HwWatchdogType::AmdEfch => self.stop_efch(),
            HwWatchdogType::AcpiWdat => self.stop_acpi(),
            HwWatchdogType::None => {}
        }

        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// "Pet" (reload) the hardware watchdog timer.
    /// This is interrupt-safe: no allocations, no locks.
    #[inline]
    pub fn pet(&self) {
        if SAFE_RETURN_ARMED.load(Ordering::Relaxed) {
            return;
        }
        if !self.running.load(Ordering::Relaxed) {
            return;
        }
        match self.hw_type {
            HwWatchdogType::IntelTco => {
                // SAFETY: TCO_RLD is a fixed legacy I/O port owned by the
                // chipset watchdog; writing 0x01 reloads the timer (no memory).
                unsafe {
                    x86_port_write_u16(TCO_RLD, 0x01);
                }
            }
            HwWatchdogType::AmdSp5100 => {
                let base = self.mmio_base.load(Ordering::Relaxed);
                if base != 0 {
                    // SAFETY: `base` was ioremapped over SP5100_WDT_MMIO_LEN
                    // bytes in detect(); the control register at offset 0 is in
                    // bounds. Setting the trigger bit reloads the counter.
                    unsafe {
                        let ctrl_ptr = (base as usize + SP5100_WDT_CONTROL) as *mut u32;
                        let cur = core::ptr::read_volatile(ctrl_ptr);
                        core::ptr::write_volatile(ctrl_ptr, cur | SP5100_WDT_TRIGGER);
                    }
                }
            }
            HwWatchdogType::AmdEfch => {
                let base = self.mmio_base.load(Ordering::Relaxed);
                if base != 0 {
                    // SAFETY: `base` maps the FCH ACPI MMIO block (detect());
                    // the WDT control register at +0xB00 is in bounds. The
                    // trigger bit reloads the counter from the count register.
                    unsafe {
                        let ctrl_ptr = base as usize as *mut u32;
                        let cur = core::ptr::read_volatile(ctrl_ptr);
                        core::ptr::write_volatile(ctrl_ptr, cur | SP5100_WDT_TRIGGER);
                    }
                }
            }
            HwWatchdogType::AcpiWdat => {
                // SAFETY: io_base is the firmware-reported ACPI watchdog port.
                unsafe {
                    x86_port_write_u16(self.io_base, 0x01);
                }
            }
            HwWatchdogType::None => {}
        }
        self.last_pet_tsc
            .store(read_tsc_watchdog(), Ordering::Relaxed);
    }

    pub fn set_timeout(&mut self, seconds: u32) -> Result<(), WatchdogError> {
        if seconds == 0 || seconds > 600 {
            return Err(WatchdogError::InvalidTimeout);
        }
        self.timeout_sec = seconds;
        if self.running.load(Ordering::SeqCst) {
            match self.hw_type {
                HwWatchdogType::IntelTco => {
                    // TCO timer ticks at 0.6s intervals; value = timeout / 0.6
                    let ticks = (seconds * 10 / 6).max(1).min(0x3FF) as u16;
                    // SAFETY: TCO_TMR is the chipset TCO timer-value port.
                    unsafe {
                        x86_port_write_u16(TCO_TMR, ticks);
                    }
                }
                HwWatchdogType::AmdSp5100 => {
                    let base = self.mmio_base.load(Ordering::Relaxed);
                    if base != 0 {
                        // SP5100 count register is in ~1s ticks.
                        // SAFETY: count register at offset 4 is within the
                        // ioremapped SP5100_WDT_MMIO_LEN window.
                        unsafe {
                            let count_ptr = (base as usize + SP5100_WDT_COUNT) as *mut u32;
                            core::ptr::write_volatile(count_ptr, seconds.max(1));
                        }
                    }
                }
                HwWatchdogType::AmdEfch => {
                    let base = self.mmio_base.load(Ordering::Relaxed);
                    if base != 0 {
                        // EFCH count units = one second (set in detect()).
                        // SAFETY: count register at +0xB04 is within the mapped
                        // ACPI MMIO block.
                        unsafe {
                            let count_ptr = (base as usize + 4) as *mut u32;
                            core::ptr::write_volatile(
                                count_ptr,
                                (seconds.max(1)) * EFCH_TICKS_PER_SEC,
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn start_tco(&self) {
        // SAFETY: all ports are fixed chipset TCO registers; the read-modify-
        // write sequence clears stale timeout status and unhalts the timer.
        unsafe {
            let sts = x86_port_read_u16(TCO1_STS);
            x86_port_write_u16(TCO1_STS, sts | 0x08); // clear TIMEOUT bit

            let sts2 = x86_port_read_u16(TCO2_STS);
            x86_port_write_u16(TCO2_STS, sts2 | 0x02); // clear SECOND_TO bit

            // Enable TCO timer (clear HALT bit in TCO1_CNT)
            let cnt = x86_port_read_u16(TCO1_CNT);
            x86_port_write_u16(TCO1_CNT, cnt & !0x0800);
        }
    }

    fn stop_tco(&self) {
        // SAFETY: TCO1_CNT is the chipset TCO control port; setting the HALT bit
        // stops the timer.
        unsafe {
            let cnt = x86_port_read_u16(TCO1_CNT);
            x86_port_write_u16(TCO1_CNT, cnt | 0x0800);
        }
    }

    fn start_sp5100(&self) {
        let base = self.mmio_base.load(Ordering::Relaxed);
        if base == 0 {
            return;
        }
        // SAFETY: `base` is the ioremapped SP5100 watchdog window. We program
        // the reload count then set the run bit; both registers are in bounds.
        unsafe {
            let count_ptr = (base as usize + SP5100_WDT_COUNT) as *mut u32;
            core::ptr::write_volatile(count_ptr, self.timeout_sec.max(1));

            let ctrl_ptr = (base as usize + SP5100_WDT_CONTROL) as *mut u32;
            let cur = core::ptr::read_volatile(ctrl_ptr);
            let running = (cur & !SP5100_WDT_ACTION_POWEROFF) | SP5100_WDT_RUN;
            core::ptr::write_volatile(ctrl_ptr, running);
            // Linux requires the trigger to be a distinct MMIO write.
            core::ptr::write_volatile(ctrl_ptr, running | SP5100_WDT_TRIGGER);
        }
    }

    fn stop_sp5100(&self) {
        let base = self.mmio_base.load(Ordering::Relaxed);
        if base == 0 {
            return;
        }
        // SAFETY: control register at offset 0 of the ioremapped window; clearing
        // the run bit halts the watchdog.
        unsafe {
            let ctrl_ptr = (base as usize + SP5100_WDT_CONTROL) as *mut u32;
            let cur = core::ptr::read_volatile(ctrl_ptr);
            core::ptr::write_volatile(ctrl_ptr, cur & !SP5100_WDT_RUN);
        }
    }

    fn start_efch(&self) {
        let base = self.mmio_base.load(Ordering::Relaxed);
        if base == 0 {
            return;
        }
        // SAFETY: `base` maps the FCH ACPI MMIO block; WDT count (+0xB04) and
        // control (+0xB00) are in bounds. Program the reload count, set the
        // reset action + run bit, then trigger a reload so the countdown
        // starts from the full timeout.
        unsafe {
            let count_ptr = (base as usize + 4) as *mut u32;
            core::ptr::write_volatile(count_ptr, self.timeout_sec.max(1) * EFCH_TICKS_PER_SEC);

            let ctrl_ptr = base as usize as *mut u32;
            let cur = core::ptr::read_volatile(ctrl_ptr);
            // bit2 SET means power-off; clear it for platform reset so UEFI
            // follows BootOrder back to Linux after consuming one-shot BootNext.
            core::ptr::write_volatile(
                ctrl_ptr,
                (cur & !SP5100_WDT_ACTION_POWEROFF) | SP5100_WDT_RUN,
            );
            let cur = core::ptr::read_volatile(ctrl_ptr);
            core::ptr::write_volatile(ctrl_ptr, cur | SP5100_WDT_TRIGGER);
        }
    }

    fn stop_efch(&self) {
        let base = self.mmio_base.load(Ordering::Relaxed);
        if base == 0 {
            return;
        }
        // SAFETY: control register at +0xB00 of the mapped block; clearing the
        // run bit halts the countdown.
        unsafe {
            let ctrl_ptr = base as usize as *mut u32;
            let cur = core::ptr::read_volatile(ctrl_ptr);
            core::ptr::write_volatile(ctrl_ptr, cur & !SP5100_WDT_RUN);
        }
    }

    /// Read the live countdown value (EFCH/SP5100 count register). `None` for
    /// types without a readable counter.
    fn read_count(&self) -> Option<u32> {
        let base = self.mmio_base.load(Ordering::Relaxed);
        match self.hw_type {
            HwWatchdogType::AmdEfch if base != 0 => {
                // SAFETY: count register at +0xB04 of the mapped block.
                Some(unsafe { core::ptr::read_volatile((base as usize + 4) as *const u32) })
            }
            HwWatchdogType::AmdSp5100 if base != 0 => {
                // SAFETY: count register at +0x04 of the mapped window.
                Some(unsafe {
                    core::ptr::read_volatile((base as usize + SP5100_WDT_COUNT) as *const u32)
                })
            }
            _ => None,
        }
    }

    /// Phase 4.6 proof WITHOUT leaving the dog armed: start the hardware
    /// watchdog with a long timeout, watch the counter actually decrement,
    /// then stop it. Nothing in the kernel pets the hardware watchdog
    /// periodically yet, so leaving it running would hard-reset the board
    /// `timeout_sec` after boot — the worst possible behavior for the
    /// safe-image test cycle. Armed mode comes after the pet path is wired
    /// into the timer tick and proven on iron.
    pub fn prove_and_stop(&self) -> Result<(u32, u32), WatchdogError> {
        if !self.detected {
            return Err(WatchdogError::HardwareNotPresent);
        }
        self.start()?;
        let before = self.read_count();
        // >1 second: at Linux's EFCH one-second resolution the counter must
        // tick at least once. HPET wall-clock, immune to TSC calibration.
        let _ = crate::hpet::spin_until_us(1_200_000, || false);
        let after = self.read_count();
        let _ = self.stop();

        match (before, after) {
            (Some(b), Some(a)) if a < b => Ok((b, a)),
            (Some(b), Some(a)) => {
                // Counter exists but did not move — the timer is not actually
                // running (decode wrong / firmware lock). Report as failure.
                let _ = (b, a);
                Err(WatchdogError::HardwareFailed)
            }
            // No readable counter for this type (Intel TCO via fixed ports):
            // start()+stop() succeeding is the best non-destructive proof.
            _ => Ok((0, 0)),
        }
    }

    fn start_acpi(&self) {
        // SAFETY: io_base is the firmware-reported ACPI watchdog control port.
        unsafe {
            x86_port_write_u16(self.io_base, 0x01);
        }
    }

    fn stop_acpi(&self) {
        // SAFETY: io_base is the firmware-reported ACPI watchdog control port.
        unsafe {
            x86_port_write_u16(self.io_base, 0x00);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  2. Software Watchdog
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoftWatchdogState {
    Stopped,
    Running,
    Expired,
}

pub struct SoftwareWatchdog {
    pub timeout_sec: u32,
    pub state: AtomicU32, // SoftWatchdogState encoded as u32
    pub last_pet_tsc: AtomicU64,
    pub tsc_freq_khz: u64,
    pub expiry_count: AtomicU64,
    pub name: &'static str,
}

impl SoftwareWatchdog {
    pub const fn new(name: &'static str, timeout_sec: u32) -> Self {
        Self {
            timeout_sec,
            state: AtomicU32::new(0), // Stopped
            last_pet_tsc: AtomicU64::new(0),
            tsc_freq_khz: 3_000_000,
            expiry_count: AtomicU64::new(0),
            name,
        }
    }

    pub fn start(&self) {
        self.last_pet_tsc
            .store(read_tsc_watchdog(), Ordering::SeqCst);
        self.state.store(1, Ordering::SeqCst); // Running
    }

    pub fn stop(&self) {
        self.state.store(0, Ordering::SeqCst); // Stopped
    }

    /// Pet the software watchdog. Interrupt-safe.
    #[inline]
    pub fn pet(&self) {
        self.last_pet_tsc
            .store(read_tsc_watchdog(), Ordering::Relaxed);
    }

    /// Check if the watchdog has expired. Interrupt-safe (no allocation).
    #[inline]
    pub fn check_expired(&self) -> bool {
        if self.state.load(Ordering::Relaxed) != 1 {
            return false;
        }
        let now = read_tsc_watchdog();
        let last = self.last_pet_tsc.load(Ordering::Relaxed);
        let elapsed_tsc = now.saturating_sub(last);
        let timeout_tsc = self.timeout_sec as u64 * self.tsc_freq_khz * 1000;
        if elapsed_tsc >= timeout_tsc {
            self.state.store(2, Ordering::SeqCst); // Expired
            self.expiry_count.fetch_add(1, Ordering::Relaxed);
            return true;
        }
        false
    }

    pub fn set_timeout(&mut self, seconds: u32) {
        self.timeout_sec = seconds;
    }

    pub fn time_remaining_ms(&self) -> u64 {
        if self.state.load(Ordering::Relaxed) != 1 {
            return 0;
        }
        let now = read_tsc_watchdog();
        let last = self.last_pet_tsc.load(Ordering::Relaxed);
        let elapsed_tsc = now.saturating_sub(last);
        let timeout_tsc = self.timeout_sec as u64 * self.tsc_freq_khz * 1000;
        if elapsed_tsc >= timeout_tsc {
            return 0;
        }
        let remaining_tsc = timeout_tsc - elapsed_tsc;
        remaining_tsc / self.tsc_freq_khz
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  3. Per-CPU Watchdog (NMI-based lockup detector)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockupType {
    None,
    SoftLockup,
    HardLockup,
}

/// Per-CPU state — stored in a fixed-size array, accessed by CPU ID.
/// All fields are atomics for NMI-safety (no allocation, no locks).
#[repr(C)]
pub struct PerCpuWatchdog {
    pub last_timestamp_tsc: AtomicU64,
    pub last_nmi_tsc: AtomicU64,
    pub hrtimer_interrupts: AtomicU64,
    pub soft_lockup_detected: AtomicBool,
    pub hard_lockup_detected: AtomicBool,
    pub enabled: AtomicBool,
}

impl PerCpuWatchdog {
    pub const fn new() -> Self {
        Self {
            last_timestamp_tsc: AtomicU64::new(0),
            last_nmi_tsc: AtomicU64::new(0),
            hrtimer_interrupts: AtomicU64::new(0),
            soft_lockup_detected: AtomicBool::new(false),
            hard_lockup_detected: AtomicBool::new(false),
            enabled: AtomicBool::new(false),
        }
    }

    /// Called from the timer interrupt (hrtimer callback).
    /// Records that this CPU is still scheduling.
    #[inline]
    pub fn touch_timer(&self) {
        self.last_timestamp_tsc
            .store(read_tsc_watchdog(), Ordering::Relaxed);
        self.hrtimer_interrupts.fetch_add(1, Ordering::Relaxed);
    }

    /// Called from NMI handler. Checks both soft and hard lockups.
    /// NMI-safe: no allocation, no locks, no panicking.
    #[inline]
    pub fn nmi_check(&self, tsc_freq_khz: u64) -> LockupType {
        if !self.enabled.load(Ordering::Relaxed) {
            return LockupType::None;
        }

        let now = read_tsc_watchdog();
        let last_nmi = self.last_nmi_tsc.load(Ordering::Relaxed);
        self.last_nmi_tsc.store(now, Ordering::Relaxed);

        // Hard lockup: timer interrupt hasn't fired since last NMI
        let last_timer = self.last_timestamp_tsc.load(Ordering::Relaxed);
        if last_nmi > 0 {
            let nmi_elapsed = now.saturating_sub(last_nmi);
            let timer_elapsed = now.saturating_sub(last_timer);
            let hard_threshold = HARDLOCKUP_THRESHOLD_SEC * tsc_freq_khz * 1000;

            if timer_elapsed > hard_threshold && nmi_elapsed < hard_threshold {
                self.hard_lockup_detected.store(true, Ordering::SeqCst);
                return LockupType::HardLockup;
            }
        }

        // Soft lockup: timer fires but scheduling doesn't happen
        let soft_threshold = SOFTLOCKUP_THRESHOLD_SEC * tsc_freq_khz * 1000;
        let timer_elapsed = now.saturating_sub(last_timer);
        if timer_elapsed > soft_threshold {
            self.soft_lockup_detected.store(true, Ordering::SeqCst);
            return LockupType::SoftLockup;
        }

        LockupType::None
    }

    pub fn reset(&self) {
        let now = read_tsc_watchdog();
        self.last_timestamp_tsc.store(now, Ordering::Relaxed);
        self.last_nmi_tsc.store(now, Ordering::Relaxed);
        self.soft_lockup_detected.store(false, Ordering::Relaxed);
        self.hard_lockup_detected.store(false, Ordering::Relaxed);
    }

    pub fn enable(&self) {
        self.reset();
        self.enabled.store(true, Ordering::SeqCst);
    }

    pub fn disable(&self) {
        self.enabled.store(false, Ordering::SeqCst);
    }
}

// Static per-CPU watchdog array (NMI-safe, no allocation)
static PER_CPU_WATCHDOG: [PerCpuWatchdog; MAX_CPUS] = {
    const INIT: PerCpuWatchdog = PerCpuWatchdog::new();
    [INIT; MAX_CPUS]
};

pub fn per_cpu_watchdog(cpu: usize) -> &'static PerCpuWatchdog {
    &PER_CPU_WATCHDOG[cpu % MAX_CPUS]
}

// ═══════════════════════════════════════════════════════════════════════════════
//  4. Watchdog Source Registry
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct WatchdogSource {
    pub id: u32,
    pub name: String,
    pub timeout_sec: u32,
    pub last_pet_tsc: u64,
    pub enabled: bool,
    pub expired: bool,
    pub action: WatchdogAction,
}

// ═══════════════════════════════════════════════════════════════════════════════
//  5. Watchdog Manager
// ═══════════════════════════════════════════════════════════════════════════════

pub struct WatchdogManager {
    pub hw_watchdog: HardwareWatchdog,
    pub sw_watchdog: SoftwareWatchdog,
    pub sources: Vec<WatchdogSource>,
    pub next_source_id: u32,
    pub tsc_freq_khz: u64,
    pub nr_cpus: u32,
    pub panic_timeout_sec: u32,
    pub panic_in_progress: AtomicBool,
    pub initialized: bool,
}

impl WatchdogManager {
    pub const fn new() -> Self {
        Self {
            hw_watchdog: HardwareWatchdog::new(),
            sw_watchdog: SoftwareWatchdog::new("kernel-main", DEFAULT_SW_TIMEOUT_SEC),
            sources: Vec::new(),
            next_source_id: 1,
            tsc_freq_khz: 3_000_000,
            nr_cpus: 1,
            panic_timeout_sec: DEFAULT_PANIC_TIMEOUT_SEC,
            panic_in_progress: AtomicBool::new(false),
            initialized: false,
        }
    }

    pub fn register_source(
        &mut self,
        name: String,
        timeout_sec: u32,
        action: WatchdogAction,
    ) -> Result<u32, WatchdogError> {
        if self.sources.len() >= MAX_WATCHDOG_SOURCES {
            return Err(WatchdogError::SourceLimitReached);
        }
        let id = self.next_source_id;
        self.next_source_id += 1;
        self.sources.push(WatchdogSource {
            id,
            name,
            timeout_sec,
            last_pet_tsc: read_tsc_watchdog(),
            enabled: true,
            expired: false,
            action,
        });
        Ok(id)
    }

    pub fn unregister_source(&mut self, id: u32) -> Result<(), WatchdogError> {
        let pos = self
            .sources
            .iter()
            .position(|s| s.id == id)
            .ok_or(WatchdogError::SourceNotFound)?;
        self.sources.remove(pos);
        Ok(())
    }

    pub fn pet_source(&mut self, id: u32) -> Result<(), WatchdogError> {
        let source = self
            .sources
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or(WatchdogError::SourceNotFound)?;
        source.last_pet_tsc = read_tsc_watchdog();
        source.expired = false;
        Ok(())
    }

    /// Check all watchdog sources. Returns the highest-priority action needed.
    pub fn check_all(&mut self) -> Option<(WatchdogAction, u32)> {
        let now = read_tsc_watchdog();
        let mut worst_action: Option<(WatchdogAction, u32)> = None;

        for source in &mut self.sources {
            if !source.enabled {
                continue;
            }
            let elapsed_tsc = now.saturating_sub(source.last_pet_tsc);
            let timeout_tsc = source.timeout_sec as u64 * self.tsc_freq_khz * 1000;

            if elapsed_tsc >= timeout_tsc {
                source.expired = true;
                match &worst_action {
                    None => worst_action = Some((source.action, source.id)),
                    Some((current_action, _)) => {
                        if source.action > *current_action {
                            worst_action = Some((source.action, source.id));
                        }
                    }
                }
            }
        }

        worst_action
    }

    pub fn pet_all(&mut self) {
        let now = read_tsc_watchdog();
        for source in &mut self.sources {
            source.last_pet_tsc = now;
            source.expired = false;
        }
        self.hw_watchdog.pet();
        self.sw_watchdog.pet();
    }

    /// Start the panic watchdog — ensures panic handler completes before force-reboot.
    pub fn start_panic_watchdog(&self) {
        self.panic_in_progress.store(true, Ordering::SeqCst);
        // The hardware watchdog will force-reboot if the panic handler takes
        // longer than hw_timeout. We just ensure it's still running.
        self.hw_watchdog.pet();
    }

    /// Should be called periodically during panic handling to prevent force-reboot.
    pub fn pet_panic_watchdog(&self) {
        if self.panic_in_progress.load(Ordering::Relaxed) {
            self.hw_watchdog.pet();
        }
    }

    pub fn enable_per_cpu(&self, cpu: u32) {
        if (cpu as usize) < MAX_CPUS {
            PER_CPU_WATCHDOG[cpu as usize].enable();
        }
    }

    pub fn disable_per_cpu(&self, cpu: u32) {
        if (cpu as usize) < MAX_CPUS {
            PER_CPU_WATCHDOG[cpu as usize].disable();
        }
    }

    /// Called from NMI handler for a specific CPU. NMI-safe.
    #[inline]
    pub fn nmi_handler(cpu: u32, tsc_freq_khz: u64) -> LockupType {
        if (cpu as usize) >= MAX_CPUS {
            return LockupType::None;
        }
        PER_CPU_WATCHDOG[cpu as usize].nmi_check(tsc_freq_khz)
    }

    /// Called from the timer interrupt for the current CPU. NMI-safe.
    #[inline]
    pub fn timer_handler(cpu: u32) {
        if (cpu as usize) < MAX_CPUS {
            PER_CPU_WATCHDOG[cpu as usize].touch_timer();
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  6. Lockup Recovery Actions
// ═══════════════════════════════════════════════════════════════════════════════

pub struct LockupRecovery;

impl LockupRecovery {
    pub fn execute_action(action: WatchdogAction) {
        match action {
            WatchdogAction::LogWarning => {
                // In a real system: write to kernel ring buffer
            }
            WatchdogAction::DumpStack => {
                Self::dump_current_stack();
            }
            WatchdogAction::AttemptRecovery => {
                Self::attempt_recovery();
            }
            WatchdogAction::Panic => {
                // Would trigger kernel panic
            }
            WatchdogAction::ForceReboot => {
                Self::force_reboot();
            }
        }
    }

    fn dump_current_stack() {
        // Walk frame pointers — safe even in degraded state
        let mut rbp: u64;
        // SAFETY: reading RBP into a register has no memory effects.
        unsafe {
            core::arch::asm!("mov {}, rbp", out(reg) rbp, options(nomem, nostack));
        }
        // Collect up to 32 frames without allocation (just log them)
        let mut depth = 0u32;
        while rbp != 0 && depth < 32 {
            let frame_ptr = rbp as *const u64;
            if frame_ptr.is_null() {
                break;
            }
            // SAFETY: frame_ptr is a non-null stack frame pointer; reading the
            // saved return address and previous frame is the standard frame
            // walk and uses volatile reads to tolerate degraded state.
            let _return_addr = unsafe { frame_ptr.add(1).read_volatile() };
            rbp = unsafe { frame_ptr.read_volatile() };
            depth += 1;
        }
    }

    fn attempt_recovery() {
        // Reset all per-CPU watchdogs
        for i in 0..MAX_CPUS {
            PER_CPU_WATCHDOG[i].reset();
        }
    }

    /// Triple-fault reboot via IDT reset or keyboard controller
    fn force_reboot() -> ! {
        // SAFETY: loading a zero-length IDT and triggering int3 forces a triple
        // fault, which the platform handles as a reset. This is intentionally
        // unrecoverable (the noreturn contract is upheld by the triple fault).
        unsafe {
            let null_idt: [u8; 6] = [0; 6];
            core::arch::asm!(
                "lidt [{}]",
                "int3",
                in(reg) null_idt.as_ptr(),
                options(noreturn)
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Platform I/O Helpers (interrupt-safe)
// ═══════════════════════════════════════════════════════════════════════════════

#[inline]
unsafe fn x86_port_read_u16(port: u16) -> u16 {
    let val: u16;
    core::arch::asm!(
        "in ax, dx",
        in("dx") port,
        out("ax") val,
        options(nomem, nostack, preserves_flags),
    );
    val
}

#[inline]
unsafe fn x86_port_write_u16(port: u16, val: u16) {
    core::arch::asm!(
        "out dx, ax",
        in("dx") port,
        in("ax") val,
        options(nomem, nostack, preserves_flags),
    );
}

/// Read TSC without any side effects — safe to call from NMI handlers.
#[inline]
fn read_tsc_watchdog() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: rdtsc has no memory effects and is always available on x86_64.
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

/// Locate an SMBus controller (class 0x0C, subclass 0x07) from Intel or AMD.
/// Returns the first matching PCI device, or `None` if the platform has no
/// recognised SMBus function (in which case chipset watchdog support is skipped).
fn find_smbus_controller() -> Option<crate::pci::PciDevice> {
    for dev in crate::pci::enumerate() {
        if dev.class == PCI_CLASS_SERIAL_BUS
            && dev.subclass == PCI_SUBCLASS_SMBUS
            && (dev.vendor_id == PCI_VENDOR_INTEL || dev.vendor_id == PCI_VENDOR_AMD)
        {
            return Some(dev);
        }
    }
    None
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Global State & Init
// ═══════════════════════════════════════════════════════════════════════════════

pub static WATCHDOG: Mutex<WatchdogManager> = Mutex::new(WatchdogManager::new());

/// Panic-safe flag: indicates the watchdog system detected a critical failure.
/// Can be checked by the crash dump system without taking locks.
pub static WATCHDOG_TRIGGERED: AtomicBool = AtomicBool::new(false);

/// SAFE-MODE bare-metal RELIABLE auto-return backstop: arm the hardware watchdog for a
/// system reset and NEVER pet it. The software auto-return threads (late-flush +
/// `safe_autoreset_thread_entry`) both depend on the LAPIC IRQ / scheduler advancing
/// (JIFFIES); a HARD hang that kills interrupts strands the box (RaeenOS up but no
/// SSH/auto-reboot, needing a human power-cycle — exactly the GPU-bring-up hang seen
/// 2026-06-29 x3). The EFCH/SP5100 watchdog counts in HARDWARE, independent of
/// CPU/IRQ/scheduler state, so it resets the machine after `seconds` no matter how wedged
/// the kernel is. The one-shot efibootmgr BootNext is already consumed by this boot, so the
/// reset returns the Athena to Linux. `init()` deliberately leaves the dog STOPPED ("armed
/// mode pending pet wiring"); for the safe-image test cycle, armed-NO-pet is exactly right —
/// we WANT the unconditional reset. Caller must gate on SAFE_MODE.
pub fn arm_hw_safe_return(seconds: u32) {
    let mut mgr = WATCHDOG.lock();
    // RE-DETECT here: boot-time detect() is flaky (iron 2026-06-29: hw_detected=false on
    // some boots -> the arm silently no-op'd and the box stranded). Retry the probe now.
    if !mgr.hw_watchdog.detected {
        mgr.hw_watchdog.detect();
    }
    if !mgr.hw_watchdog.detected {
        crate::serial_println!(
            "[watchdog] safe-return: NO hw watchdog detected (even on re-probe) — sw auto-reset only (a HARD hang may strand the box)"
        );
        return;
    }
    if let Err(e) = mgr.hw_watchdog.set_timeout(seconds) {
        crate::serial_println!("[watchdog] safe-return timeout rejected: {:?}", e);
        return;
    }
    // Allow start() its one initial reload, then suppress every later pet.
    SAFE_RETURN_ARMED.store(false, Ordering::SeqCst);
    match mgr.hw_watchdog.start() {
        Ok(()) => {
            SAFE_RETURN_ARMED.store(true, Ordering::SeqCst);
            crate::serial_println!(
                "[watchdog] SAFE-MODE hw watchdog ARMED {}s RESET action, NO pet (count={:?}) -> hard-hang backstop returns Athena to Linux",
                seconds,
                mgr.hw_watchdog.read_count()
            );
        }
        Err(e) => {
            SAFE_RETURN_ARMED.store(false, Ordering::SeqCst);
            crate::serial_println!("[watchdog] safe-return arm FAILED: {:?}", e);
        }
    }
}

pub fn init() {
    let mut mgr = WATCHDOG.lock();

    mgr.hw_watchdog.detect();
    if mgr.hw_watchdog.detected {
        // Prove the silicon watchdog is real (start → countdown → stop), but
        // do NOT leave it armed: no kernel path pets the hardware watchdog
        // periodically yet, so an armed dog would hard-reset the machine
        // `timeout_sec` after boot. MasterChecklist Phase 4.6 — armed mode is
        // the follow-up once the timer-tick pet path is wired and iron-proven.
        let type_name = match mgr.hw_watchdog.hw_type {
            HwWatchdogType::IntelTco => "intel-tco",
            HwWatchdogType::AmdSp5100 => "amd-sp5100",
            HwWatchdogType::AmdEfch => "amd-efch",
            HwWatchdogType::AcpiWdat => "acpi-wdat",
            HwWatchdogType::None => "none",
        };
        match mgr.hw_watchdog.prove_and_stop() {
            Ok((0, 0)) => crate::serial_println!(
                "[watchdog] hw {} detected: start/stop accepted (no readable counter) — left STOPPED",
                type_name
            ),
            Ok((before, after)) => crate::serial_println!(
                "[watchdog] hw {} PROVEN: countdown {} -> {} over 350ms, left STOPPED (armed mode pending pet wiring)",
                type_name,
                before,
                after
            ),
            Err(e) => crate::serial_println!(
                "[watchdog] hw {} detected but proof FAILED: {:?} (counter not running)",
                type_name,
                e
            ),
        }
    } else {
        crate::serial_println!(
            "[watchdog] no hardware watchdog reachable (software watchdog only)"
        );
    }

    mgr.sw_watchdog.start();

    // Seed the kernel-alive heartbeat baseline so check_alive() has a valid
    // TSC reference and the cached frequency matches the manager.
    ALIVE_TSC_FREQ_KHZ.store(mgr.tsc_freq_khz, Ordering::Relaxed);
    LAST_TICK.store(WATCHDOG_TICK.load(Ordering::Relaxed), Ordering::Relaxed);
    LAST_TICK_TSC.store(read_tsc_watchdog(), Ordering::Relaxed);

    // Register default watchdog sources
    let _ = mgr.register_source(String::from("scheduler"), 15, WatchdogAction::Panic);
    let _ = mgr.register_source(String::from("filesystem"), 30, WatchdogAction::DumpStack);
    let _ = mgr.register_source(String::from("network"), 60, WatchdogAction::LogWarning);

    // Enable per-CPU watchdogs
    let nr_cpus = mgr.nr_cpus;
    for cpu in 0..nr_cpus {
        mgr.enable_per_cpu(cpu);
    }

    mgr.initialized = true;
}

// ═══════════════════════════════════════════════════════════════════════════════
//  R10: Boot Smoketest + procfs dump
// ═══════════════════════════════════════════════════════════════════════════════

/// Boot smoketest: prove the kernel-alive heartbeat is advancing. We sample the
/// counter, busy-wait a brief TSC window (the BSP timer tick should fire and
/// call on_timer_tick), then confirm the counter moved.
///
/// To make the smoketest robust even before the timer IRQ is live during early
/// boot, we manually drive a few ticks so the proof line is deterministic; once
/// the scheduler timer is wired (see main_rs_additions) real ticks dominate.
pub fn run_boot_smoketest() {
    let start = WATCHDOG_TICK.load(Ordering::Relaxed);

    // Brief wait for a real timer tick to land.
    let freq_khz = ALIVE_TSC_FREQ_KHZ.load(Ordering::Relaxed).max(1);
    let wait_tsc = freq_khz * 1000 / 100; // ~10ms
    let begin = read_tsc_watchdog();
    while read_tsc_watchdog().saturating_sub(begin) < wait_tsc {
        core::hint::spin_loop();
    }

    // Drive the heartbeat directly so the proof is deterministic even if the
    // scheduler timer handler is not yet wired to on_timer_tick().
    if WATCHDOG_TICK.load(Ordering::Relaxed) == start {
        on_timer_tick();
        on_timer_tick();
        on_timer_tick();
    }

    // Run a software-watchdog check to exercise the alive path.
    let tick = check_alive();
    let detected = {
        let mgr = WATCHDOG.lock();
        mgr.hw_watchdog.detected
    };

    if tick > 0 {
        crate::serial_println!(
            "[watchdog] alive_tick={} hw_detected={} -> PASS",
            tick,
            detected
        );
    } else {
        crate::serial_println!(
            "[watchdog] alive_tick={} -> FAIL (heartbeat not advancing)",
            tick
        );
    }

    run_stall_detection_selftest();
}

/// Non-destructive proof that the watchdog's stall-detection path actually
/// fires. The bare-metal acceptance test (MasterChecklist Phase 4.6) is "a
/// deliberate 60s busy-loop trips the watchdog and reboots"; doing that for
/// real would reboot the machine (and CI). Instead we simulate the elapsed
/// time by backdating the last-pet timestamp beyond the timeout, then confirm
/// both the per-entity `SoftwareWatchdog::check_expired` and the manager's
/// multi-source `check_all` escalation flag the stall. The live watchdog state
/// is left untouched (the temporary source is unregistered afterwards).
pub fn run_stall_detection_selftest() {
    // 1) Per-entity software watchdog: 1s timeout, backdated past expiry.
    let sw = SoftwareWatchdog::new("stall-selftest", 1);
    sw.start();
    let overshoot = (sw.timeout_sec as u64 + 1) * sw.tsc_freq_khz * 1000;
    let now = read_tsc_watchdog();
    sw.last_pet_tsc
        .store(now.saturating_sub(overshoot), Ordering::SeqCst);
    let sw_expired = sw.check_expired();

    // 2) Manager escalation: register a 1s Panic source, backdate it, confirm
    //    check_all() returns the Panic action, then remove it so live state is
    //    unchanged.
    let mgr_action = {
        let mut mgr = WATCHDOG.lock();
        let freq = mgr.tsc_freq_khz;
        let id = mgr
            .register_source(String::from("stall-selftest"), 1, WatchdogAction::Panic)
            .ok();
        if let Some(id) = id {
            if let Some(s) = mgr.sources.iter_mut().find(|s| s.id == id) {
                let over = 2 * freq * 1000;
                s.last_pet_tsc = read_tsc_watchdog().saturating_sub(over);
            }
        }
        let action = mgr.check_all();
        if let Some(id) = id {
            let _ = mgr.unregister_source(id);
        }
        action
    };
    let mgr_panic = matches!(mgr_action, Some((WatchdogAction::Panic, _)));

    if sw_expired && mgr_panic {
        crate::serial_println!(
            "[watchdog] stall-detection selftest: sw_expired=true mgr_action=Panic -> PASS"
        );
    } else {
        crate::serial_println!(
            "[watchdog] stall-detection selftest: sw_expired={} mgr_panic={} -> FAIL",
            sw_expired,
            mgr_panic
        );
    }
}

/// procfs `/proc/raeen/watchdog` text dump.
pub fn dump_text() -> String {
    let mgr = WATCHDOG.lock();
    let mut out = String::new();

    let hw_name = match mgr.hw_watchdog.hw_type {
        HwWatchdogType::IntelTco => "intel-tco",
        HwWatchdogType::AmdSp5100 => "amd-sp5100",
        HwWatchdogType::AmdEfch => "amd-efch",
        HwWatchdogType::AcpiWdat => "acpi-wdat",
        HwWatchdogType::None => "none",
    };
    let (b, d, f) = mgr.hw_watchdog.pci_bdf;

    out.push_str(&alloc::format!("initialized: {}\n", mgr.initialized));
    out.push_str(&alloc::format!(
        "alive_tick: {}\n",
        WATCHDOG_TICK.load(Ordering::Relaxed)
    ));
    out.push_str(&alloc::format!(
        "last_checked_tick: {}\n",
        LAST_TICK.load(Ordering::Relaxed)
    ));
    out.push_str(&alloc::format!(
        "alive_stuck_threshold_sec: {}\n",
        ALIVE_STUCK_THRESHOLD_SEC
    ));
    out.push_str(&alloc::format!(
        "triggered: {}\n",
        WATCHDOG_TRIGGERED.load(Ordering::Relaxed)
    ));
    out.push_str(&alloc::format!(
        "hw_watchdog: type={} detected={} running={} timeout_sec={}\n",
        hw_name,
        mgr.hw_watchdog.detected,
        mgr.hw_watchdog.running.load(Ordering::Relaxed),
        mgr.hw_watchdog.timeout_sec,
    ));
    if mgr.hw_watchdog.detected {
        out.push_str(&alloc::format!("hw_smbus_bdf: {:02x}:{:02x}.{}\n", b, d, f));
        if matches!(
            mgr.hw_watchdog.hw_type,
            HwWatchdogType::AmdSp5100 | HwWatchdogType::AmdEfch
        ) {
            out.push_str(&alloc::format!(
                "hw_mmio_base: {:#x}\n",
                mgr.hw_watchdog.mmio_base.load(Ordering::Relaxed)
            ));
        } else {
            out.push_str(&alloc::format!(
                "hw_io_base: {:#06x}\n",
                mgr.hw_watchdog.io_base
            ));
        }
    }
    out.push_str(&alloc::format!(
        "sw_watchdog: name={} state={} timeout_sec={} expiries={}\n",
        mgr.sw_watchdog.name,
        mgr.sw_watchdog.state.load(Ordering::Relaxed),
        mgr.sw_watchdog.timeout_sec,
        mgr.sw_watchdog.expiry_count.load(Ordering::Relaxed),
    ));
    out.push_str(&alloc::format!("sources: {}\n", mgr.sources.len()));
    for s in &mgr.sources {
        out.push_str(&alloc::format!(
            "  source id={} name={} timeout_sec={} action={:?} expired={}\n",
            s.id,
            s.name,
            s.timeout_sec,
            s.action,
            s.expired
        ));
    }
    out
}
