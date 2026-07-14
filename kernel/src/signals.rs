// kernel/src/signals.rs
//
// POSIX signal-manager *placeholder*.
//
// Concept §"From-scratch hybrid microkernel, MIT-licensed, Rust+NASM":
//   POSIX is an interop surface, not the native ABI. The full 2105-line
//   signals implementation was removed in commit b568b6e as part of a
//   dead-code sweep; the callers in posix.rs were left behind. This stub
//   restores the exact symbol surface that posix.rs imports so the kernel
//   compiles and boots, while making every signal/timer call a no-op or
//   trivial success.
//
// Honest status (R15 — no bullshit):
//   • SIGNAL_MANAGER is always Some(SignalManager::empty()) so callers
//     that branch on `is_some()` walk the no-op path.
//   • sys_kill / sys_sigaction / sys_sigprocmask / sys_sigsuspend all
//     return Ok(()). When real POSIX matters, restore the deleted module.
//   • clock_gettime falls through to the kernel's monotonic clock so
//     `gettimeofday` returns increasing values instead of always zero.

#![allow(dead_code)]

use spin::Mutex;

#[derive(Debug, Clone, Copy)]
pub enum SignalError {
    InvalidSignal,
    InvalidArgument,
    NoSuchProcess,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SignalSet(pub u64);

#[derive(Debug, Clone, Copy, Default)]
pub struct SignalHandler {
    pub handler: u64,
    pub mask: SignalSet,
    pub flags: u32,
}

#[derive(Debug, Clone, Copy)]
pub enum SigProcMaskHow {
    Block,
    Unblock,
    SetMask,
}

impl SigProcMaskHow {
    pub fn from_raw(raw: u32) -> Result<Self, ()> {
        match raw {
            0 => Ok(Self::Block),
            1 => Ok(Self::Unblock),
            2 => Ok(Self::SetMask),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockId {
    Realtime,
    Monotonic,
    ProcessCpu,
    ThreadCpu,
}

impl ClockId {
    pub fn from_raw(raw: u32) -> Result<Self, ()> {
        match raw {
            0 => Ok(Self::Realtime),
            1 => Ok(Self::Monotonic),
            2 => Ok(Self::ProcessCpu),
            3 => Ok(Self::ThreadCpu),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Timespec {
    pub sec: i64,
    pub nsec: i64,
}

impl Timespec {
    pub fn new(sec: i64, nsec: i64) -> Self {
        Self { sec, nsec }
    }
}

pub struct TimerManager;

impl TimerManager {
    pub const fn new() -> Self {
        Self
    }

    /// Returns a wall-clock Timespec.
    ///   • Realtime → derived from CMOS RTC at boot + TSC delta (rtc.rs)
    ///   • Monotonic / *Cpu → same source for now; we lack a true monotonic
    ///     clock that survives suspend, which the real implementation owes.
    pub fn clock_gettime(&self, _clk: ClockId) -> Timespec {
        let ns = crate::rtc::nanos_since_epoch_now();
        let sec = (ns / 1_000_000_000) as i64;
        let nsec = (ns % 1_000_000_000) as i64;
        Timespec { sec, nsec }
    }

    /// "Sleep" — placeholder. Returns Ok(()) so callers don't loop.
    pub fn nanosleep(&self, _req: &Timespec) -> Result<(), Timespec> {
        Ok(())
    }
}

pub struct SignalManager {
    pub timer_manager: TimerManager,
}

impl SignalManager {
    pub const fn empty() -> Self {
        Self {
            timer_manager: TimerManager::new(),
        }
    }

    pub fn fork_process(&mut self, _parent: u64, _child: u64) {}
    pub fn exec_process(&mut self, _pid: u64) {}
}

pub static SIGNAL_MANAGER: Mutex<Option<SignalManager>> = Mutex::new(None);

pub fn init(_init_pid: u64) {
    *SIGNAL_MANAGER.lock() = Some(SignalManager::empty());
    crate::serial_println!("[signals] placeholder manager online (no-op POSIX surface)");
}

pub fn run_boot_smoketest() {
    let g = SIGNAL_MANAGER.lock();
    if g.is_some() {
        crate::serial_println!("[signals] boot smoketest: manager present (stub)");
    } else {
        crate::serial_println!("[signals] boot smoketest: manager MISSING");
    }
}

pub fn sys_kill(_pid: i64, _sig: u8) -> Result<(), SignalError> {
    Ok(())
}

pub fn sys_sigaction(
    _sig: u8,
    _act: Option<&SignalHandler>,
    _oldact: Option<&mut SignalHandler>,
) -> Result<(), SignalError> {
    Ok(())
}

pub fn sys_sigprocmask(
    _how: SigProcMaskHow,
    _set: Option<&SignalSet>,
    _oldset: Option<&mut SignalSet>,
) -> Result<(), SignalError> {
    Ok(())
}

pub fn sys_sigsuspend(_mask: &SignalSet) -> Result<(), SignalError> {
    Ok(())
}
