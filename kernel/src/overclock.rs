//! OS-level overclocking API — replaces MSI Afterburner / Armoury Crate / iCUE sprawl.
//!
//! Provides unified CPU, GPU, and memory overclock control with safety guardrails,
//! profile management, stress testing, and per-game automatic profile application.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

use crate::cpufreq::{read_msr, write_msr};

// ═══════════════════════════════════════════════════════════════════════════════
//  MSR Constants for Overclocking
// ═══════════════════════════════════════════════════════════════════════════════

const MSR_PLATFORM_INFO: u32 = 0xCE;
const IA32_PERF_CTL: u32 = 0x199;
const IA32_PERF_STATUS: u32 = 0x198;
const MSR_TURBO_RATIO_LIMIT: u32 = 0x1AD;
const MSR_TURBO_RATIO_LIMIT1: u32 = 0x1AE;
const MSR_VOLTAGE_ID: u32 = 0x198;
const MSR_OC_MAILBOX: u32 = 0x150;
const MSR_FLEX_RATIO: u32 = 0x194;
const MSR_POWER_CTL: u32 = 0x1FC;
const MSR_MISC_PWR_MGMT: u32 = 0x1AA;
const MSR_RAPL_POWER_UNIT: u32 = 0x606;
const MSR_PKG_POWER_LIMIT: u32 = 0x610;
const MSR_PP0_POWER_LIMIT: u32 = 0x638;
const MSR_TEMPERATURE_TARGET: u32 = 0x1A2;

// AMD-specific MSRs
const AMD_MSR_PSTATE_0: u32 = 0xC001_0064;
const AMD_MSR_PSTATE_LIMIT: u32 = 0xC001_0061;
const AMD_MSR_PSTATE_CTL: u32 = 0xC001_0062;
const AMD_MSR_PSTATE_STATUS: u32 = 0xC001_0063;
const AMD_MSR_HWCR: u32 = 0xC001_0015;

// Voltage guardrails (millivolts)
const CPU_MAX_VOLTAGE_OFFSET_MV: i32 = 200;
const CPU_MIN_VOLTAGE_OFFSET_MV: i32 = -150;
const GPU_MAX_VOLTAGE_OFFSET_MV: i32 = 150;
const GPU_MIN_VOLTAGE_OFFSET_MV: i32 = -100;
const ABSOLUTE_MAX_CPU_VOLTAGE_MV: u32 = 1550;
const ABSOLUTE_MAX_GPU_VOLTAGE_MV: u32 = 1200;

// ═══════════════════════════════════════════════════════════════════════════════
//  Error Types
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverclockError {
    VoltageOutOfRange,
    FrequencyOutOfRange,
    ThermalLimitExceeded,
    PowerLimitExceeded,
    ProfileNotFound,
    ProfileAlreadyExists,
    StressTestFailed,
    MsrAccessDenied,
    HardwareNotSupported,
    AutoRevertTriggered,
    InvalidParameter,
    NotInitialized,
    CpuNotDetected,
    GpuNotDetected,
    MemoryControllerUnavailable,
}

// ═══════════════════════════════════════════════════════════════════════════════
//  CPU Vendor Detection
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuVendor {
    Intel,
    Amd,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuIdentity {
    pub vendor: CpuVendor,
    pub family: u8,
    pub model: u8,
    pub stepping: u8,
    pub max_ratio: u8,
    pub min_ratio: u8,
    pub oc_capable: bool,
}

pub fn detect_cpu() -> CpuIdentity {
    let (ebx, _, _) = cpuid(0);
    let vendor = if ebx == 0x756E_6547 {
        CpuVendor::Intel
    } else if ebx == 0x6874_7541 {
        CpuVendor::Amd
    } else {
        CpuVendor::Unknown
    };

    let (eax, _, _) = cpuid(1);
    let stepping = (eax & 0xF) as u8;
    let model = ((eax >> 4) & 0xF) as u8 | (((eax >> 16) & 0xF) as u8) << 4;
    let family = ((eax >> 8) & 0xF) as u8;

    let platform_info = unsafe { read_msr(MSR_PLATFORM_INFO) };
    let max_ratio = ((platform_info >> 8) & 0xFF) as u8;
    let min_ratio = ((platform_info >> 40) & 0xFF) as u8;
    let oc_capable = (platform_info >> 30) & 1 != 0;

    CpuIdentity {
        vendor,
        family,
        model,
        stepping,
        max_ratio,
        min_ratio,
        oc_capable,
    }
}

fn cpuid(leaf: u32) -> (u32, u32, u32) {
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "xor ecx, ecx",
            "cpuid",
            "mov {ebx_out:e}, ebx",
            "pop rbx",
            in("eax") leaf,
            ebx_out = out(reg) ebx,
            out("ecx") ecx,
            out("edx") edx,
            options(nostack, preserves_flags),
        );
    }
    (ebx, ecx, edx)
}

// ═══════════════════════════════════════════════════════════════════════════════
//  1. CPU Overclocking
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct CoreOverclockState {
    pub core_id: u8,
    pub multiplier: u8,
    pub voltage_offset_mv: i32,
    pub frequency_mhz: u32,
    pub temperature_mc: i32,
    pub power_draw_mw: u32,
    pub throttled: bool,
}

#[derive(Debug, Clone)]
pub struct CpuOverclock {
    pub cpu_id: CpuIdentity,
    pub cores: Vec<CoreOverclockState>,
    pub base_clock_mhz: u32,
    pub all_core_multiplier: u8,
    pub voltage_offset_mv: i32,
    pub power_limit_1_w: u32,
    pub power_limit_2_w: u32,
    pub current_limit_a: u32,
    pub pbo_enabled: bool,
    pub pbo_scalar: u8,
    pub pbo_offset_mv: i32,
    pub turbo_ratios: [u8; 8],
    pub thermal_throttle_temp_c: u8,
    pub auto_revert_active: bool,
}

impl CpuOverclock {
    pub fn new(cpu_id: CpuIdentity, core_count: u8) -> Self {
        let mut cores = Vec::with_capacity(core_count as usize);
        for i in 0..core_count {
            cores.push(CoreOverclockState {
                core_id: i,
                multiplier: cpu_id.max_ratio,
                voltage_offset_mv: 0,
                frequency_mhz: cpu_id.max_ratio as u32 * 100,
                temperature_mc: 0,
                power_draw_mw: 0,
                throttled: false,
            });
        }

        let turbo_ratios = Self::read_turbo_ratios(&cpu_id);

        Self {
            cpu_id,
            cores,
            base_clock_mhz: 100,
            all_core_multiplier: cpu_id.max_ratio,
            voltage_offset_mv: 0,
            power_limit_1_w: 125,
            power_limit_2_w: 250,
            current_limit_a: 200,
            pbo_enabled: false,
            pbo_scalar: 1,
            pbo_offset_mv: 0,
            turbo_ratios,
            thermal_throttle_temp_c: 100,
            auto_revert_active: false,
        }
    }

    fn read_turbo_ratios(cpu_id: &CpuIdentity) -> [u8; 8] {
        let mut ratios = [0u8; 8];
        if cpu_id.vendor == CpuVendor::Intel {
            let msr_val = unsafe { read_msr(MSR_TURBO_RATIO_LIMIT) };
            for i in 0..8 {
                ratios[i] = ((msr_val >> (i * 8)) & 0xFF) as u8;
            }
        }
        ratios
    }

    pub fn set_core_multiplier(&mut self, core: u8, multiplier: u8) -> Result<(), OverclockError> {
        let max_allowed = self.cpu_id.max_ratio + 15; // allow +15 bins above stock
        if multiplier < self.cpu_id.min_ratio || multiplier > max_allowed {
            return Err(OverclockError::FrequencyOutOfRange);
        }
        if let Some(c) = self.cores.get_mut(core as usize) {
            c.multiplier = multiplier;
            c.frequency_mhz = multiplier as u32 * self.base_clock_mhz;
            self.apply_core_ratio(core, multiplier);
            Ok(())
        } else {
            Err(OverclockError::InvalidParameter)
        }
    }

    pub fn set_all_core_multiplier(&mut self, multiplier: u8) -> Result<(), OverclockError> {
        let max_allowed = self.cpu_id.max_ratio + 15;
        if multiplier < self.cpu_id.min_ratio || multiplier > max_allowed {
            return Err(OverclockError::FrequencyOutOfRange);
        }
        self.all_core_multiplier = multiplier;
        for core in &mut self.cores {
            core.multiplier = multiplier;
            core.frequency_mhz = multiplier as u32 * self.base_clock_mhz;
        }
        self.apply_all_core_ratio(multiplier);
        Ok(())
    }

    pub fn set_voltage_offset(&mut self, offset_mv: i32) -> Result<(), OverclockError> {
        if offset_mv < CPU_MIN_VOLTAGE_OFFSET_MV || offset_mv > CPU_MAX_VOLTAGE_OFFSET_MV {
            return Err(OverclockError::VoltageOutOfRange);
        }
        self.voltage_offset_mv = offset_mv;
        self.apply_voltage_offset(offset_mv);
        Ok(())
    }

    pub fn set_power_limits(&mut self, pl1_w: u32, pl2_w: u32) -> Result<(), OverclockError> {
        if pl1_w > 500 || pl2_w > 1000 {
            return Err(OverclockError::PowerLimitExceeded);
        }
        self.power_limit_1_w = pl1_w;
        self.power_limit_2_w = pl2_w;
        self.apply_power_limits();
        Ok(())
    }

    pub fn set_pbo(
        &mut self,
        enabled: bool,
        scalar: u8,
        offset_mv: i32,
    ) -> Result<(), OverclockError> {
        if self.cpu_id.vendor != CpuVendor::Amd {
            return Err(OverclockError::HardwareNotSupported);
        }
        if offset_mv < CPU_MIN_VOLTAGE_OFFSET_MV || offset_mv > CPU_MAX_VOLTAGE_OFFSET_MV {
            return Err(OverclockError::VoltageOutOfRange);
        }
        self.pbo_enabled = enabled;
        self.pbo_scalar = scalar.min(10);
        self.pbo_offset_mv = offset_mv;
        Ok(())
    }

    pub fn set_turbo_ratio(&mut self, active_cores: u8, ratio: u8) -> Result<(), OverclockError> {
        if self.cpu_id.vendor != CpuVendor::Intel {
            return Err(OverclockError::HardwareNotSupported);
        }
        if active_cores == 0 || active_cores > 8 {
            return Err(OverclockError::InvalidParameter);
        }
        let max_allowed = self.cpu_id.max_ratio + 20;
        if ratio > max_allowed {
            return Err(OverclockError::FrequencyOutOfRange);
        }
        self.turbo_ratios[(active_cores - 1) as usize] = ratio;
        self.apply_turbo_ratios();
        Ok(())
    }

    fn apply_core_ratio(&self, _core: u8, ratio: u8) {
        let val = (ratio as u64) << 8;
        unsafe {
            write_msr(IA32_PERF_CTL, val);
        }
    }

    fn apply_all_core_ratio(&self, ratio: u8) {
        let val = (ratio as u64) << 8;
        unsafe {
            write_msr(IA32_PERF_CTL, val);
        }
    }

    fn apply_voltage_offset(&self, offset_mv: i32) {
        // OC Mailbox: command=0x11 (voltage offset), param=offset in 1/1024V units
        let offset_units = (offset_mv as i64 * 1024 / 1000) as u64;
        let command: u64 = 0x11;
        let mailbox_val = (1u64 << 63) | (command << 32) | (offset_units & 0xFFFF_FFFF);
        unsafe {
            write_msr(MSR_OC_MAILBOX, mailbox_val);
        }
    }

    fn apply_power_limits(&self) {
        let units = unsafe { read_msr(MSR_RAPL_POWER_UNIT) };
        let power_unit = 1u64 << (units & 0xF);

        let pl1_raw = (self.power_limit_1_w as u64 * power_unit) | (1 << 15); // enable
        let time_window_1: u64 = 0x6E; // ~28s
        let pl1 = pl1_raw | (time_window_1 << 17);

        let pl2_raw = (self.power_limit_2_w as u64 * power_unit) | (1 << 15); // enable
        let time_window_2: u64 = 0x05; // ~2.44ms
        let pl2 = pl2_raw | (time_window_2 << 17);

        let combined = pl1 | (pl2 << 32);
        unsafe {
            write_msr(MSR_PKG_POWER_LIMIT, combined);
        }
    }

    fn apply_turbo_ratios(&self) {
        if self.cpu_id.vendor != CpuVendor::Intel {
            return;
        }
        let mut val: u64 = 0;
        for (i, &ratio) in self.turbo_ratios.iter().enumerate() {
            val |= (ratio as u64) << (i * 8);
        }
        unsafe {
            write_msr(MSR_TURBO_RATIO_LIMIT, val);
        }
    }

    pub fn read_current_state(&mut self) {
        let status = unsafe { read_msr(IA32_PERF_STATUS) };
        let current_ratio = ((status >> 8) & 0xFF) as u8;
        for core in &mut self.cores {
            core.frequency_mhz = current_ratio as u32 * self.base_clock_mhz;
        }
    }

    pub fn check_thermal_safety(&self) -> bool {
        self.cores
            .iter()
            .all(|c| c.temperature_mc < (self.thermal_throttle_temp_c as i32 * 1000))
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  2. GPU Overclocking
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuChipVendor {
    Nvidia,
    Amd,
    Intel,
    Unknown,
}

#[derive(Debug, Clone, Copy)]
pub struct VoltageCurvePoint {
    pub frequency_mhz: u32,
    pub voltage_mv: u32,
}

#[derive(Debug, Clone)]
pub struct GpuOverclock {
    pub vendor: GpuChipVendor,
    pub core_clock_offset_mhz: i32,
    pub memory_clock_offset_mhz: i32,
    pub power_limit_percent: i32,
    pub temp_limit_c: u8,
    pub fan_speed_percent: Option<u8>,
    pub voltage_curve: Vec<VoltageCurvePoint>,
    pub current_core_clock_mhz: u32,
    pub current_mem_clock_mhz: u32,
    pub current_temp_c: u8,
    pub current_power_draw_w: u32,
    pub current_fan_rpm: u32,
    pub current_voltage_mv: u32,
    pub max_core_offset_mhz: i32,
    pub min_core_offset_mhz: i32,
    pub max_mem_offset_mhz: i32,
    pub max_power_limit_percent: i32,
    pub min_power_limit_percent: i32,
}

impl GpuOverclock {
    pub fn new(vendor: GpuChipVendor) -> Self {
        Self {
            vendor,
            core_clock_offset_mhz: 0,
            memory_clock_offset_mhz: 0,
            power_limit_percent: 100,
            temp_limit_c: 83,
            fan_speed_percent: None,
            voltage_curve: Vec::new(),
            current_core_clock_mhz: 0,
            current_mem_clock_mhz: 0,
            current_temp_c: 0,
            current_power_draw_w: 0,
            current_fan_rpm: 0,
            current_voltage_mv: 0,
            max_core_offset_mhz: 300,
            min_core_offset_mhz: -500,
            max_mem_offset_mhz: 1500,
            max_power_limit_percent: 130,
            min_power_limit_percent: 50,
        }
    }

    pub fn set_core_clock_offset(&mut self, offset_mhz: i32) -> Result<(), OverclockError> {
        if offset_mhz < self.min_core_offset_mhz || offset_mhz > self.max_core_offset_mhz {
            return Err(OverclockError::FrequencyOutOfRange);
        }
        self.core_clock_offset_mhz = offset_mhz;
        Ok(())
    }

    pub fn set_memory_clock_offset(&mut self, offset_mhz: i32) -> Result<(), OverclockError> {
        if offset_mhz < -500 || offset_mhz > self.max_mem_offset_mhz {
            return Err(OverclockError::FrequencyOutOfRange);
        }
        self.memory_clock_offset_mhz = offset_mhz;
        Ok(())
    }

    pub fn set_power_limit(&mut self, percent: i32) -> Result<(), OverclockError> {
        if percent < self.min_power_limit_percent || percent > self.max_power_limit_percent {
            return Err(OverclockError::PowerLimitExceeded);
        }
        self.power_limit_percent = percent;
        Ok(())
    }

    pub fn set_temp_limit(&mut self, temp_c: u8) -> Result<(), OverclockError> {
        if temp_c < 60 || temp_c > 95 {
            return Err(OverclockError::ThermalLimitExceeded);
        }
        self.temp_limit_c = temp_c;
        Ok(())
    }

    pub fn set_fan_speed(&mut self, percent: Option<u8>) -> Result<(), OverclockError> {
        if let Some(p) = percent {
            if p > 100 {
                return Err(OverclockError::InvalidParameter);
            }
        }
        self.fan_speed_percent = percent;
        Ok(())
    }

    pub fn add_voltage_curve_point(
        &mut self,
        freq_mhz: u32,
        voltage_mv: u32,
    ) -> Result<(), OverclockError> {
        if voltage_mv > ABSOLUTE_MAX_GPU_VOLTAGE_MV {
            return Err(OverclockError::VoltageOutOfRange);
        }
        self.voltage_curve.push(VoltageCurvePoint {
            frequency_mhz: freq_mhz,
            voltage_mv,
        });
        self.voltage_curve.sort_by_key(|p| p.frequency_mhz);
        Ok(())
    }

    pub fn clear_voltage_curve(&mut self) {
        self.voltage_curve.clear();
    }

    pub fn check_thermal_safety(&self) -> bool {
        self.current_temp_c < self.temp_limit_c
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  3. Memory Overclocking
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryProfile {
    Jedec,
    Xmp1,
    Xmp2,
    Xmp3,
    Expo1,
    Expo2,
    Manual,
}

#[derive(Debug, Clone, Copy)]
pub struct MemoryTimings {
    pub cl: u8,
    pub trcd: u8,
    pub trp: u8,
    pub tras: u16,
    pub trc: u16,
    pub trfc: u16,
    pub twr: u8,
    pub trrd_s: u8,
    pub trrd_l: u8,
    pub tfaw: u16,
    pub tcwl: u8,
    pub trtp: u8,
    pub command_rate: u8,
}

impl MemoryTimings {
    pub const fn default_ddr5() -> Self {
        Self {
            cl: 40,
            trcd: 40,
            trp: 40,
            tras: 76,
            trc: 116,
            trfc: 350,
            twr: 48,
            trrd_s: 8,
            trrd_l: 12,
            tfaw: 32,
            tcwl: 38,
            trtp: 12,
            command_rate: 1,
        }
    }

    pub const fn default_ddr4() -> Self {
        Self {
            cl: 16,
            trcd: 18,
            trp: 18,
            tras: 36,
            trc: 54,
            trfc: 280,
            twr: 16,
            trrd_s: 4,
            trrd_l: 6,
            tfaw: 24,
            tcwl: 14,
            trtp: 8,
            command_rate: 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DramType {
    Ddr4,
    Ddr5,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct MemoryOverclock {
    pub dram_type: DramType,
    pub active_profile: MemoryProfile,
    pub frequency_mhz: u32,
    pub voltage_mv: u32,
    pub timings: MemoryTimings,
    pub channels: u8,
    pub ranks_per_channel: u8,
    pub total_capacity_mb: u32,
    pub xmp_profiles_available: u8,
    pub expo_profiles_available: u8,
    pub gear_mode: u8,
}

impl MemoryOverclock {
    pub fn new(dram_type: DramType) -> Self {
        let timings = match dram_type {
            DramType::Ddr5 => MemoryTimings::default_ddr5(),
            DramType::Ddr4 => MemoryTimings::default_ddr4(),
            DramType::Unknown => MemoryTimings::default_ddr4(),
        };
        Self {
            dram_type,
            active_profile: MemoryProfile::Jedec,
            frequency_mhz: match dram_type {
                DramType::Ddr5 => 4800,
                DramType::Ddr4 => 2133,
                DramType::Unknown => 2133,
            },
            voltage_mv: match dram_type {
                DramType::Ddr5 => 1100,
                DramType::Ddr4 => 1200,
                DramType::Unknown => 1200,
            },
            timings,
            channels: 2,
            ranks_per_channel: 1,
            total_capacity_mb: 16384,
            xmp_profiles_available: 0,
            expo_profiles_available: 0,
            gear_mode: 1,
        }
    }

    pub fn select_profile(&mut self, profile: MemoryProfile) -> Result<(), OverclockError> {
        match profile {
            MemoryProfile::Xmp1 | MemoryProfile::Xmp2 | MemoryProfile::Xmp3 => {
                if self.xmp_profiles_available == 0 {
                    return Err(OverclockError::HardwareNotSupported);
                }
            }
            MemoryProfile::Expo1 | MemoryProfile::Expo2 => {
                if self.expo_profiles_available == 0 {
                    return Err(OverclockError::HardwareNotSupported);
                }
            }
            _ => {}
        }
        self.active_profile = profile;
        Ok(())
    }

    pub fn set_frequency(&mut self, freq_mhz: u32) -> Result<(), OverclockError> {
        let max_freq = match self.dram_type {
            DramType::Ddr5 => 8400,
            DramType::Ddr4 => 5333,
            DramType::Unknown => 3200,
        };
        if freq_mhz > max_freq || freq_mhz < 1600 {
            return Err(OverclockError::FrequencyOutOfRange);
        }
        self.frequency_mhz = freq_mhz;
        self.active_profile = MemoryProfile::Manual;
        Ok(())
    }

    pub fn set_timings(&mut self, timings: MemoryTimings) -> Result<(), OverclockError> {
        if timings.cl == 0 || timings.trcd == 0 || timings.trp == 0 {
            return Err(OverclockError::InvalidParameter);
        }
        self.timings = timings;
        self.active_profile = MemoryProfile::Manual;
        Ok(())
    }

    pub fn set_voltage(&mut self, voltage_mv: u32) -> Result<(), OverclockError> {
        let max_v = match self.dram_type {
            DramType::Ddr5 => 1450,
            DramType::Ddr4 => 1500,
            DramType::Unknown => 1350,
        };
        if voltage_mv > max_v || voltage_mv < 1000 {
            return Err(OverclockError::VoltageOutOfRange);
        }
        self.voltage_mv = voltage_mv;
        Ok(())
    }

    pub fn effective_bandwidth_mbs(&self) -> u64 {
        let data_rate = self.frequency_mhz as u64 * 2; // double data rate
        let bus_width = self.channels as u64 * 64; // 64 bits per channel
        data_rate * bus_width / 8 // bytes per second in MB/s
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  4. Overclock Profiles
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfilePreset {
    Default,
    Gaming,
    Rendering,
    PowerSaver,
    Manual,
}

#[derive(Debug, Clone)]
pub struct OverclockProfile {
    pub name: String,
    pub preset: ProfilePreset,
    pub cpu_multiplier: u8,
    pub cpu_voltage_offset_mv: i32,
    pub cpu_power_limit_w: u32,
    pub gpu_core_offset_mhz: i32,
    pub gpu_mem_offset_mhz: i32,
    pub gpu_power_limit_percent: i32,
    pub gpu_temp_limit_c: u8,
    pub memory_profile: MemoryProfile,
    pub memory_freq_mhz: u32,
    pub fan_mode_auto: bool,
    pub timestamp_created: u64,
    pub timestamp_modified: u64,
    pub game_associations: Vec<String>,
}

impl OverclockProfile {
    pub fn default_profile() -> Self {
        Self {
            name: String::from("Default"),
            preset: ProfilePreset::Default,
            cpu_multiplier: 0,
            cpu_voltage_offset_mv: 0,
            cpu_power_limit_w: 125,
            gpu_core_offset_mhz: 0,
            gpu_mem_offset_mhz: 0,
            gpu_power_limit_percent: 100,
            gpu_temp_limit_c: 83,
            memory_profile: MemoryProfile::Jedec,
            memory_freq_mhz: 0,
            fan_mode_auto: true,
            timestamp_created: 0,
            timestamp_modified: 0,
            game_associations: Vec::new(),
        }
    }

    pub fn gaming_profile() -> Self {
        Self {
            name: String::from("Gaming"),
            preset: ProfilePreset::Gaming,
            cpu_multiplier: 2,
            cpu_voltage_offset_mv: 0,
            cpu_power_limit_w: 200,
            gpu_core_offset_mhz: 150,
            gpu_mem_offset_mhz: 500,
            gpu_power_limit_percent: 115,
            gpu_temp_limit_c: 87,
            memory_profile: MemoryProfile::Xmp1,
            memory_freq_mhz: 0,
            fan_mode_auto: true,
            timestamp_created: 0,
            timestamp_modified: 0,
            game_associations: Vec::new(),
        }
    }

    pub fn rendering_profile() -> Self {
        Self {
            name: String::from("Rendering"),
            preset: ProfilePreset::Rendering,
            cpu_multiplier: 4,
            cpu_voltage_offset_mv: 25,
            cpu_power_limit_w: 250,
            gpu_core_offset_mhz: 100,
            gpu_mem_offset_mhz: 300,
            gpu_power_limit_percent: 120,
            gpu_temp_limit_c: 90,
            memory_profile: MemoryProfile::Xmp1,
            memory_freq_mhz: 0,
            fan_mode_auto: false,
            timestamp_created: 0,
            timestamp_modified: 0,
            game_associations: Vec::new(),
        }
    }

    pub fn power_saver_profile() -> Self {
        Self {
            name: String::from("Power Saver"),
            preset: ProfilePreset::PowerSaver,
            cpu_multiplier: 0,
            cpu_voltage_offset_mv: -50,
            cpu_power_limit_w: 65,
            gpu_core_offset_mhz: -200,
            gpu_mem_offset_mhz: 0,
            gpu_power_limit_percent: 70,
            gpu_temp_limit_c: 75,
            memory_profile: MemoryProfile::Jedec,
            memory_freq_mhz: 0,
            fan_mode_auto: true,
            timestamp_created: 0,
            timestamp_modified: 0,
            game_associations: Vec::new(),
        }
    }

    pub fn associate_game(&mut self, game_name: String) {
        if !self.game_associations.contains(&game_name) {
            self.game_associations.push(game_name);
        }
    }

    pub fn disassociate_game(&mut self, game_name: &str) {
        self.game_associations.retain(|g| g.as_str() != game_name);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  5. Stress Test
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StressTestType {
    CpuPrime,
    CpuLinpack,
    GpuFurmark,
    GpuCompute,
    Memory,
    Combined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StressTestStatus {
    Idle,
    Running,
    Passed,
    Failed,
    Aborted,
}

#[derive(Debug, Clone, Copy)]
pub struct StressTestResult {
    pub test_type: StressTestType,
    pub status: StressTestStatus,
    pub duration_ms: u64,
    pub max_temp_cpu_mc: i32,
    pub max_temp_gpu_mc: i32,
    pub max_power_cpu_mw: u32,
    pub max_power_gpu_mw: u32,
    pub errors_detected: u32,
    pub thermal_throttle_events: u32,
    pub avg_frequency_mhz: u32,
    pub min_frequency_mhz: u32,
}

pub struct StressTest {
    pub test_type: StressTestType,
    pub status: StressTestStatus,
    pub duration_target_ms: u64,
    pub elapsed_ms: u64,
    pub iteration_count: u64,
    pub error_count: u32,
    pub max_temp_cpu_mc: i32,
    pub max_temp_gpu_mc: i32,
    pub max_power_cpu_mw: u32,
    pub max_power_gpu_mw: u32,
    pub thermal_events: u32,
    pub freq_samples: Vec<u32>,
    pub abort_on_error: bool,
    pub abort_on_thermal: bool,
    pub thermal_abort_temp_mc: i32,
}

impl StressTest {
    pub fn new(test_type: StressTestType, duration_ms: u64) -> Self {
        Self {
            test_type,
            status: StressTestStatus::Idle,
            duration_target_ms: duration_ms,
            elapsed_ms: 0,
            iteration_count: 0,
            error_count: 0,
            max_temp_cpu_mc: 0,
            max_temp_gpu_mc: 0,
            max_power_cpu_mw: 0,
            max_power_gpu_mw: 0,
            thermal_events: 0,
            freq_samples: Vec::new(),
            abort_on_error: true,
            abort_on_thermal: true,
            thermal_abort_temp_mc: 100_000,
        }
    }

    pub fn start(&mut self) {
        self.status = StressTestStatus::Running;
        self.elapsed_ms = 0;
        self.iteration_count = 0;
        self.error_count = 0;
    }

    pub fn tick(
        &mut self,
        delta_ms: u64,
        temp_cpu_mc: i32,
        temp_gpu_mc: i32,
        power_cpu_mw: u32,
        power_gpu_mw: u32,
        freq_mhz: u32,
    ) -> bool {
        if self.status != StressTestStatus::Running {
            return false;
        }

        self.elapsed_ms += delta_ms;
        self.iteration_count += 1;

        if temp_cpu_mc > self.max_temp_cpu_mc {
            self.max_temp_cpu_mc = temp_cpu_mc;
        }
        if temp_gpu_mc > self.max_temp_gpu_mc {
            self.max_temp_gpu_mc = temp_gpu_mc;
        }
        if power_cpu_mw > self.max_power_cpu_mw {
            self.max_power_cpu_mw = power_cpu_mw;
        }
        if power_gpu_mw > self.max_power_gpu_mw {
            self.max_power_gpu_mw = power_gpu_mw;
        }
        self.freq_samples.push(freq_mhz);

        if self.abort_on_thermal && temp_cpu_mc >= self.thermal_abort_temp_mc {
            self.thermal_events += 1;
            self.status = StressTestStatus::Failed;
            return false;
        }

        if self.elapsed_ms >= self.duration_target_ms {
            self.status = if self.error_count == 0 {
                StressTestStatus::Passed
            } else {
                StressTestStatus::Failed
            };
            return false;
        }

        true
    }

    pub fn record_error(&mut self) {
        self.error_count += 1;
        if self.abort_on_error {
            self.status = StressTestStatus::Failed;
        }
    }

    pub fn abort(&mut self) {
        self.status = StressTestStatus::Aborted;
    }

    pub fn result(&self) -> StressTestResult {
        let avg_freq = if self.freq_samples.is_empty() {
            0
        } else {
            let sum: u64 = self.freq_samples.iter().map(|&f| f as u64).sum();
            (sum / self.freq_samples.len() as u64) as u32
        };
        let min_freq = self.freq_samples.iter().copied().min().unwrap_or(0);

        StressTestResult {
            test_type: self.test_type,
            status: self.status,
            duration_ms: self.elapsed_ms,
            max_temp_cpu_mc: self.max_temp_cpu_mc,
            max_temp_gpu_mc: self.max_temp_gpu_mc,
            max_power_cpu_mw: self.max_power_cpu_mw,
            max_power_gpu_mw: self.max_power_gpu_mw,
            errors_detected: self.error_count,
            thermal_throttle_events: self.thermal_events,
            avg_frequency_mhz: avg_freq,
            min_frequency_mhz: min_freq,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  6. Overclock Monitor
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct MonitorSample {
    pub timestamp_tsc: u64,
    pub cpu_freq_mhz: u32,
    pub cpu_temp_mc: i32,
    pub cpu_voltage_mv: u32,
    pub cpu_power_mw: u32,
    pub gpu_freq_mhz: u32,
    pub gpu_temp_mc: i32,
    pub gpu_voltage_mv: u32,
    pub gpu_power_mw: u32,
    pub mem_freq_mhz: u32,
}

const MONITOR_RING_SIZE: usize = 3600; // 1 hour at 1 sample/s

pub struct OverclockMonitor {
    pub samples: Vec<MonitorSample>,
    pub write_index: usize,
    pub sample_count: u64,
    pub sample_interval_ms: u32,
    pub last_sample_tsc: u64,
    pub peak_cpu_temp_mc: i32,
    pub peak_gpu_temp_mc: i32,
    pub peak_cpu_power_mw: u32,
    pub peak_gpu_power_mw: u32,
}

impl OverclockMonitor {
    pub fn new(interval_ms: u32) -> Self {
        let mut samples = Vec::with_capacity(MONITOR_RING_SIZE);
        for _ in 0..MONITOR_RING_SIZE {
            samples.push(MonitorSample {
                timestamp_tsc: 0,
                cpu_freq_mhz: 0,
                cpu_temp_mc: 0,
                cpu_voltage_mv: 0,
                cpu_power_mw: 0,
                gpu_freq_mhz: 0,
                gpu_temp_mc: 0,
                gpu_voltage_mv: 0,
                gpu_power_mw: 0,
                mem_freq_mhz: 0,
            });
        }
        Self {
            samples,
            write_index: 0,
            sample_count: 0,
            sample_interval_ms: interval_ms,
            last_sample_tsc: 0,
            peak_cpu_temp_mc: 0,
            peak_gpu_temp_mc: 0,
            peak_cpu_power_mw: 0,
            peak_gpu_power_mw: 0,
        }
    }

    pub fn record(&mut self, sample: MonitorSample) {
        if sample.cpu_temp_mc > self.peak_cpu_temp_mc {
            self.peak_cpu_temp_mc = sample.cpu_temp_mc;
        }
        if sample.gpu_temp_mc > self.peak_gpu_temp_mc {
            self.peak_gpu_temp_mc = sample.gpu_temp_mc;
        }
        if sample.cpu_power_mw > self.peak_cpu_power_mw {
            self.peak_cpu_power_mw = sample.cpu_power_mw;
        }
        if sample.gpu_power_mw > self.peak_gpu_power_mw {
            self.peak_gpu_power_mw = sample.gpu_power_mw;
        }

        self.samples[self.write_index] = sample;
        self.write_index = (self.write_index + 1) % MONITOR_RING_SIZE;
        self.sample_count += 1;
        self.last_sample_tsc = sample.timestamp_tsc;
    }

    pub fn latest(&self) -> Option<&MonitorSample> {
        if self.sample_count == 0 {
            return None;
        }
        let idx = if self.write_index == 0 {
            MONITOR_RING_SIZE - 1
        } else {
            self.write_index - 1
        };
        Some(&self.samples[idx])
    }

    pub fn average_cpu_temp(&self, last_n: usize) -> i32 {
        let count = last_n
            .min(self.sample_count as usize)
            .min(MONITOR_RING_SIZE);
        if count == 0 {
            return 0;
        }
        let mut sum: i64 = 0;
        for i in 0..count {
            let idx = (self.write_index + MONITOR_RING_SIZE - 1 - i) % MONITOR_RING_SIZE;
            sum += self.samples[idx].cpu_temp_mc as i64;
        }
        (sum / count as i64) as i32
    }

    pub fn reset_peaks(&mut self) {
        self.peak_cpu_temp_mc = 0;
        self.peak_gpu_temp_mc = 0;
        self.peak_cpu_power_mw = 0;
        self.peak_gpu_power_mw = 0;
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  7. Overclock History (Audit Log)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverclockAction {
    CpuMultiplierChanged,
    CpuVoltageChanged,
    CpuPowerLimitChanged,
    GpuCoreOffsetChanged,
    GpuMemOffsetChanged,
    GpuPowerLimitChanged,
    MemoryProfileChanged,
    MemoryFreqChanged,
    ProfileApplied,
    AutoRevertTriggered,
    StressTestStarted,
    StressTestCompleted,
    ThermalThrottleDetected,
}

#[derive(Debug, Clone)]
pub struct OverclockHistoryEntry {
    pub timestamp_tsc: u64,
    pub action: OverclockAction,
    pub old_value: i64,
    pub new_value: i64,
    pub profile_name: Option<String>,
    pub success: bool,
}

const HISTORY_MAX_ENTRIES: usize = 256;

pub struct OverclockHistory {
    pub entries: Vec<OverclockHistoryEntry>,
}

impl OverclockHistory {
    pub fn new() -> Self {
        Self {
            entries: Vec::with_capacity(HISTORY_MAX_ENTRIES),
        }
    }

    pub fn record(&mut self, action: OverclockAction, old_val: i64, new_val: i64, success: bool) {
        let tsc = read_tsc();
        if self.entries.len() >= HISTORY_MAX_ENTRIES {
            self.entries.remove(0);
        }
        self.entries.push(OverclockHistoryEntry {
            timestamp_tsc: tsc,
            action,
            old_value: old_val,
            new_value: new_val,
            profile_name: None,
            success,
        });
    }

    pub fn record_profile_change(&mut self, profile: &str) {
        let tsc = read_tsc();
        if self.entries.len() >= HISTORY_MAX_ENTRIES {
            self.entries.remove(0);
        }
        self.entries.push(OverclockHistoryEntry {
            timestamp_tsc: tsc,
            action: OverclockAction::ProfileApplied,
            old_value: 0,
            new_value: 0,
            profile_name: Some(String::from(profile)),
            success: true,
        });
    }

    pub fn last_n(&self, n: usize) -> &[OverclockHistoryEntry] {
        let start = self.entries.len().saturating_sub(n);
        &self.entries[start..]
    }

    pub fn last_failure(&self) -> Option<&OverclockHistoryEntry> {
        self.entries.iter().rev().find(|e| !e.success)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  8. Auto-Revert Safety System
// ═══════════════════════════════════════════════════════════════════════════════

pub struct AutoRevert {
    pub armed: AtomicBool,
    pub countdown_tsc: AtomicU64,
    pub revert_timeout_ms: u64,
    pub tsc_freq_khz: u64,
}

impl AutoRevert {
    pub const fn new() -> Self {
        Self {
            armed: AtomicBool::new(false),
            countdown_tsc: AtomicU64::new(0),
            revert_timeout_ms: 10_000,
            tsc_freq_khz: 3_000_000, // 3 GHz default, calibrated at boot
        }
    }

    pub fn arm(&self) {
        let deadline = read_tsc() + (self.revert_timeout_ms * self.tsc_freq_khz);
        self.countdown_tsc.store(deadline, Ordering::SeqCst);
        self.armed.store(true, Ordering::SeqCst);
    }

    pub fn disarm(&self) {
        self.armed.store(false, Ordering::SeqCst);
    }

    pub fn is_armed(&self) -> bool {
        self.armed.load(Ordering::SeqCst)
    }

    pub fn check_expired(&self) -> bool {
        if !self.is_armed() {
            return false;
        }
        let now = read_tsc();
        let deadline = self.countdown_tsc.load(Ordering::SeqCst);
        now >= deadline
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  9. Global Overclock System
// ═══════════════════════════════════════════════════════════════════════════════

pub struct OverclockSystem {
    pub cpu: Option<CpuOverclock>,
    pub gpu: Option<GpuOverclock>,
    pub memory: Option<MemoryOverclock>,
    pub profiles: Vec<OverclockProfile>,
    pub active_profile_index: usize,
    pub monitor: OverclockMonitor,
    pub history: OverclockHistory,
    pub initialized: bool,
}

impl OverclockSystem {
    pub const fn new() -> Self {
        Self {
            cpu: None,
            gpu: None,
            memory: None,
            profiles: Vec::new(),
            active_profile_index: 0,
            monitor: OverclockMonitor {
                samples: Vec::new(),
                write_index: 0,
                sample_count: 0,
                sample_interval_ms: 1000,
                last_sample_tsc: 0,
                peak_cpu_temp_mc: 0,
                peak_gpu_temp_mc: 0,
                peak_cpu_power_mw: 0,
                peak_gpu_power_mw: 0,
            },
            history: OverclockHistory {
                entries: Vec::new(),
            },
            initialized: false,
        }
    }

    pub fn init_cpu(&mut self, core_count: u8) {
        let cpu_id = detect_cpu();
        self.cpu = Some(CpuOverclock::new(cpu_id, core_count));
    }

    pub fn init_gpu(&mut self, vendor: GpuChipVendor) {
        self.gpu = Some(GpuOverclock::new(vendor));
    }

    pub fn init_memory(&mut self, dram_type: DramType) {
        self.memory = Some(MemoryOverclock::new(dram_type));
    }

    pub fn add_profile(&mut self, profile: OverclockProfile) -> Result<(), OverclockError> {
        if self.profiles.iter().any(|p| p.name == profile.name) {
            return Err(OverclockError::ProfileAlreadyExists);
        }
        self.profiles.push(profile);
        Ok(())
    }

    pub fn apply_profile(&mut self, name: &str) -> Result<(), OverclockError> {
        let idx = self
            .profiles
            .iter()
            .position(|p| p.name.as_str() == name)
            .ok_or(OverclockError::ProfileNotFound)?;

        let profile = self.profiles[idx].clone();

        if let Some(ref mut cpu) = self.cpu {
            if profile.cpu_multiplier > 0 {
                let target = cpu.cpu_id.max_ratio + profile.cpu_multiplier;
                let _ = cpu.set_all_core_multiplier(target);
            }
            let _ = cpu.set_voltage_offset(profile.cpu_voltage_offset_mv);
            let _ = cpu.set_power_limits(profile.cpu_power_limit_w, profile.cpu_power_limit_w * 2);
        }

        if let Some(ref mut gpu) = self.gpu {
            let _ = gpu.set_core_clock_offset(profile.gpu_core_offset_mhz);
            let _ = gpu.set_memory_clock_offset(profile.gpu_mem_offset_mhz);
            let _ = gpu.set_power_limit(profile.gpu_power_limit_percent);
            let _ = gpu.set_temp_limit(profile.gpu_temp_limit_c);
        }

        if let Some(ref mut mem) = self.memory {
            let _ = mem.select_profile(profile.memory_profile);
            if profile.memory_freq_mhz > 0 {
                let _ = mem.set_frequency(profile.memory_freq_mhz);
            }
        }

        self.active_profile_index = idx;
        self.history.record_profile_change(name);
        Ok(())
    }

    pub fn apply_game_profile(&mut self, game_name: &str) -> Result<(), OverclockError> {
        let profile_name = self
            .profiles
            .iter()
            .find(|p| p.game_associations.iter().any(|g| g.as_str() == game_name))
            .map(|p| p.name.clone());

        if let Some(name) = profile_name {
            self.apply_profile(&name)
        } else {
            Err(OverclockError::ProfileNotFound)
        }
    }

    pub fn revert_to_default(&mut self) {
        if let Some(ref mut cpu) = self.cpu {
            let default_ratio = cpu.cpu_id.max_ratio;
            let _ = cpu.set_all_core_multiplier(default_ratio);
            let _ = cpu.set_voltage_offset(0);
            let _ = cpu.set_power_limits(125, 250);
        }
        if let Some(ref mut gpu) = self.gpu {
            let _ = gpu.set_core_clock_offset(0);
            let _ = gpu.set_memory_clock_offset(0);
            let _ = gpu.set_power_limit(100);
        }
        if let Some(ref mut mem) = self.memory {
            let _ = mem.select_profile(MemoryProfile::Jedec);
        }
        self.history
            .record(OverclockAction::AutoRevertTriggered, 0, 0, true);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TSC Helper
// ═══════════════════════════════════════════════════════════════════════════════

#[inline]
pub fn read_tsc() -> u64 {
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

pub static OVERCLOCK: Mutex<OverclockSystem> = Mutex::new(OverclockSystem::new());
pub static AUTO_REVERT: AutoRevert = AutoRevert::new();

pub fn init() {
    let mut sys = OVERCLOCK.lock();
    sys.init_cpu(4);
    sys.init_gpu(GpuChipVendor::Unknown);
    sys.init_memory(DramType::Ddr5);
    sys.monitor = OverclockMonitor::new(1000);

    let _ = sys.add_profile(OverclockProfile::default_profile());
    let _ = sys.add_profile(OverclockProfile::gaming_profile());
    let _ = sys.add_profile(OverclockProfile::rendering_profile());
    let _ = sys.add_profile(OverclockProfile::power_saver_profile());

    sys.initialized = true;
}
