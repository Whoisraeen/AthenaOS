#![allow(dead_code)]

extern crate alloc;

use crate::arch::VirtAddr;
use alloc::vec::Vec;
use spin::Mutex;

use crate::xhci_desc::HidInterruptEndpoint;

fn v2p<T>(ptr: *const T) -> u64 {
    let virt = VirtAddr::new(ptr as u64);
    crate::memory::virt_to_phys(virt)
        .map(|p| p.as_u64())
        .unwrap_or(0)
}

/// Single 4 KiB page from the frame allocator (DMA-visible to the host controller).
#[derive(Debug)]
pub struct DmaPage {
    phys: u64,
    virt: u64,
}

fn alloc_dma_page() -> Result<DmaPage, XhciError> {
    use x86_64::structures::paging::FrameAllocator;
    let mut alloc = crate::memory::GlobalFrameAllocator;
    let frame = alloc.allocate_frame().ok_or(XhciError::NoMemory)?;
    let phys = frame.start_address().as_u64();
    let virt = crate::memory::phys_to_virt(phys).as_u64();
    // Safety: freshly allocated frame mapped in the kernel heap window.
    unsafe {
        core::ptr::write_bytes(virt as *mut u8, 0, 4096);
    }
    Ok(DmaPage { phys, virt })
}

impl DmaPage {
    /// Return this page's frame to the buddy allocator. CONSUMING and **only**
    /// safe for a TRANSIENT page whose DMA is COMPLETE — the one-shot control-IN
    /// data buffers (`get_descriptor` / `get_hid_report_descriptor` /
    /// `control_in_vec`) and the isoch test-tone scratch (`play_test_tone`),
    /// each called after `wait_for_transfer` returned and the result was copied
    /// out. NEVER call it on a page still referenced by hardware — a command /
    /// transfer / event ring, the scratchpad, a device input/output context, an
    /// armed HID `report_buf`, or a buffer behind an in-flight TD: that is a
    /// use-after-free, not a leak (the xHC keeps DMAing to those long after the
    /// building call returns; cf. `disable_slot`, which drops a DeviceSlot
    /// without awaiting the Disable Slot command or clearing its DCBAA entry).
    /// This is deliberately NOT a `Drop` impl on `DmaPage` for that reason — a
    /// blanket Drop would free those persistent pages the moment their owner
    /// dropped. The rare control-transfer timeout/stall paths intentionally leak
    /// (bounded per boot) rather than free a buffer a pending TD may still target.
    fn free(self) {
        use crate::arch::PhysAddr;
        use x86_64::structures::paging::PhysFrame;
        crate::memory::deallocate_frame(PhysFrame::containing_address(PhysAddr::new(self.phys)));
    }
}

/// Fill a 1 ms PCM buffer (48 kHz, 16-bit, stereo) with a 440 Hz square wave,
/// continuing from sample `phase`; returns the next phase so consecutive buffers
/// form a seamless tone across isochronous service intervals. 1 ms = 48 frames ×
/// 2 channels × 2 bytes = 192 bytes. Stand-in for the RaeAudio mixer output until
/// the USB DAC is wired as a RaeAudio sink (MasterChecklist Phase 2.6/7).
fn fill_square_tone(buf: &mut [u8], mut phase: u32) -> u32 {
    const PERIOD: u32 = 48_000 / 440; // ≈109 samples per 440 Hz cycle
    let mut i = 0;
    while i + 4 <= buf.len() {
        let amp: i16 = if phase % PERIOD < PERIOD / 2 {
            6000
        } else {
            -6000
        };
        let s = amp.to_le_bytes();
        buf[i] = s[0]; // left lo
        buf[i + 1] = s[1]; // left hi
        buf[i + 2] = s[0]; // right lo
        buf[i + 3] = s[1]; // right hi
        i += 4;
        phase = phase.wrapping_add(1);
    }
    phase
}

// ─── xHCI Capability Registers ──────────────────────────────────────────────

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CapabilityRegisters {
    pub caplength: u8,
    pub reserved: u8,
    pub hciversion: u16,
    pub hcsparams1: u32,
    pub hcsparams2: u32,
    pub hcsparams3: u32,
    pub hccparams1: u32,
    pub dboff: u32,
    pub rtsoff: u32,
    pub hccparams2: u32,
}

impl CapabilityRegisters {
    pub fn max_slots(&self) -> u8 {
        (self.hcsparams1 & 0xFF) as u8
    }

    pub fn max_intrs(&self) -> u16 {
        ((self.hcsparams1 >> 8) & 0x7FF) as u16
    }

    pub fn max_ports(&self) -> u8 {
        ((self.hcsparams1 >> 24) & 0xFF) as u8
    }

    pub fn ist(&self) -> u8 {
        (self.hcsparams2 & 0x0F) as u8
    }

    pub fn erst_max(&self) -> u8 {
        ((self.hcsparams2 >> 4) & 0x0F) as u8
    }

    pub fn max_scratchpad_bufs_hi(&self) -> u8 {
        ((self.hcsparams2 >> 21) & 0x1F) as u8
    }

    pub fn max_scratchpad_bufs_lo(&self) -> u8 {
        ((self.hcsparams2 >> 27) & 0x1F) as u8
    }

    pub fn max_scratchpad_bufs(&self) -> u32 {
        ((self.max_scratchpad_bufs_hi() as u32) << 5) | self.max_scratchpad_bufs_lo() as u32
    }

    pub fn u1_device_exit_latency(&self) -> u8 {
        (self.hcsparams3 & 0xFF) as u8
    }

    pub fn u2_device_exit_latency(&self) -> u16 {
        ((self.hcsparams3 >> 16) & 0xFFFF) as u16
    }

    pub fn ac64(&self) -> bool {
        self.hccparams1 & (1 << 0) != 0
    }
    pub fn bnc(&self) -> bool {
        self.hccparams1 & (1 << 1) != 0
    }
    pub fn csz(&self) -> bool {
        self.hccparams1 & (1 << 2) != 0
    }
    pub fn ppc(&self) -> bool {
        self.hccparams1 & (1 << 3) != 0
    }
    pub fn pind(&self) -> bool {
        self.hccparams1 & (1 << 4) != 0
    }
    pub fn lhrc(&self) -> bool {
        self.hccparams1 & (1 << 5) != 0
    }
    pub fn ltc(&self) -> bool {
        self.hccparams1 & (1 << 6) != 0
    }
    pub fn nss(&self) -> bool {
        self.hccparams1 & (1 << 7) != 0
    }
    pub fn pae(&self) -> bool {
        self.hccparams1 & (1 << 8) != 0
    }
    pub fn spc(&self) -> bool {
        self.hccparams1 & (1 << 9) != 0
    }
    pub fn sec(&self) -> bool {
        self.hccparams1 & (1 << 10) != 0
    }
    pub fn cfc(&self) -> bool {
        self.hccparams1 & (1 << 11) != 0
    }
    pub fn max_psa_size(&self) -> u8 {
        ((self.hccparams1 >> 12) & 0x0F) as u8
    }
    pub fn xecp(&self) -> u16 {
        ((self.hccparams1 >> 16) & 0xFFFF) as u16
    }

    pub fn u3_entry_capable(&self) -> bool {
        self.hccparams2 & (1 << 0) != 0
    }
    pub fn configure_endpoint_commands_max(&self) -> bool {
        self.hccparams2 & (1 << 1) != 0
    }
    pub fn force_save_context(&self) -> bool {
        self.hccparams2 & (1 << 2) != 0
    }
    pub fn compliance_transition(&self) -> bool {
        self.hccparams2 & (1 << 3) != 0
    }
    pub fn large_esit_payload(&self) -> bool {
        self.hccparams2 & (1 << 4) != 0
    }
    pub fn configuration_information(&self) -> bool {
        self.hccparams2 & (1 << 5) != 0
    }
    pub fn extended_tbc(&self) -> bool {
        self.hccparams2 & (1 << 6) != 0
    }
    pub fn extended_tbc_trb_status(&self) -> bool {
        self.hccparams2 & (1 << 7) != 0
    }
    pub fn get_set_extended_property(&self) -> bool {
        self.hccparams2 & (1 << 8) != 0
    }
    pub fn virtualization_based_trusted_io(&self) -> bool {
        self.hccparams2 & (1 << 9) != 0
    }

    pub fn doorbell_array_offset(&self) -> u32 {
        self.dboff & !0x03
    }
    pub fn runtime_register_space_offset(&self) -> u32 {
        self.rtsoff & !0x1F
    }
}

// ─── xHCI Operational Registers ─────────────────────────────────────────────

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OperationalRegisters {
    pub usbcmd: u32,
    pub usbsts: u32,
    pub pagesize: u32,
    pub reserved1: [u32; 2],
    pub dnctrl: u32,
    pub crcr: u64,
    pub reserved2: [u32; 4],
    pub dcbaap: u64,
    pub config: u32,
}

pub const USBCMD_RS: u32 = 1 << 0;
pub const USBCMD_HCRST: u32 = 1 << 1;
pub const USBCMD_INTE: u32 = 1 << 2;
pub const USBCMD_HSEE: u32 = 1 << 3;
pub const USBCMD_LHCRST: u32 = 1 << 7;
pub const USBCMD_CSS: u32 = 1 << 8;
pub const USBCMD_CRS: u32 = 1 << 9;
pub const USBCMD_EWE: u32 = 1 << 10;
pub const USBCMD_EU3S: u32 = 1 << 11;
pub const USBCMD_CME: u32 = 1 << 13;

pub const USBSTS_HCH: u32 = 1 << 0;
pub const USBSTS_HSE: u32 = 1 << 2;
pub const USBSTS_EINT: u32 = 1 << 3;
pub const USBSTS_PCD: u32 = 1 << 4;
pub const USBSTS_SSS: u32 = 1 << 8;
pub const USBSTS_RSS: u32 = 1 << 9;
pub const USBSTS_SRE: u32 = 1 << 10;
pub const USBSTS_CNR: u32 = 1 << 11;
pub const USBSTS_HCE: u32 = 1 << 12;

// ─── Port Registers ─────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PortRegisters {
    pub portsc: u32,
    pub portpmsc: u32,
    pub portli: u32,
    pub porthlpmc: u32,
}

pub const PORTSC_CCS: u32 = 1 << 0;
pub const PORTSC_PED: u32 = 1 << 1;
pub const PORTSC_OCA: u32 = 1 << 3;
pub const PORTSC_PR: u32 = 1 << 4;
pub const PORTSC_PP: u32 = 1 << 9;
pub const PORTSC_LWS: u32 = 1 << 16;
pub const PORTSC_CSC: u32 = 1 << 17;
pub const PORTSC_PEC: u32 = 1 << 18;
pub const PORTSC_WRC: u32 = 1 << 19;
pub const PORTSC_OCC: u32 = 1 << 20;
pub const PORTSC_PRC: u32 = 1 << 21;
pub const PORTSC_PLC: u32 = 1 << 22;
pub const PORTSC_CEC: u32 = 1 << 23;
pub const PORTSC_CAS: u32 = 1 << 24;
pub const PORTSC_WCE: u32 = 1 << 25;
pub const PORTSC_WDE: u32 = 1 << 26;
pub const PORTSC_WOE: u32 = 1 << 27;

pub const PORTSC_PLS_MASK: u32 = 0x0F << 5;
pub const PORTSC_SPEED_MASK: u32 = 0x0F << 10;
pub const PORTSC_PIC_MASK: u32 = 0x03 << 14;

impl PortRegisters {
    pub fn connected(&self) -> bool {
        self.portsc & PORTSC_CCS != 0
    }
    pub fn enabled(&self) -> bool {
        self.portsc & PORTSC_PED != 0
    }
    pub fn over_current(&self) -> bool {
        self.portsc & PORTSC_OCA != 0
    }
    pub fn reset_active(&self) -> bool {
        self.portsc & PORTSC_PR != 0
    }
    pub fn port_power(&self) -> bool {
        self.portsc & PORTSC_PP != 0
    }

    pub fn port_link_state(&self) -> u8 {
        ((self.portsc & PORTSC_PLS_MASK) >> 5) as u8
    }

    pub fn port_speed(&self) -> PortSpeed {
        match (self.portsc & PORTSC_SPEED_MASK) >> 10 {
            1 => PortSpeed::FullSpeed,
            2 => PortSpeed::LowSpeed,
            3 => PortSpeed::HighSpeed,
            4 => PortSpeed::SuperSpeed,
            5 => PortSpeed::SuperSpeedPlus,
            _ => PortSpeed::Undefined,
        }
    }

    pub fn port_indicator(&self) -> u8 {
        ((self.portsc & PORTSC_PIC_MASK) >> 14) as u8
    }

    pub fn connection_status_change(&self) -> bool {
        self.portsc & PORTSC_CSC != 0
    }
    pub fn port_enabled_change(&self) -> bool {
        self.portsc & PORTSC_PEC != 0
    }
    pub fn warm_reset_change(&self) -> bool {
        self.portsc & PORTSC_WRC != 0
    }
    pub fn over_current_change(&self) -> bool {
        self.portsc & PORTSC_OCC != 0
    }
    pub fn port_reset_change(&self) -> bool {
        self.portsc & PORTSC_PRC != 0
    }
    pub fn port_link_state_change(&self) -> bool {
        self.portsc & PORTSC_PLC != 0
    }
    pub fn port_config_error_change(&self) -> bool {
        self.portsc & PORTSC_CEC != 0
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortSpeed {
    Undefined = 0,
    FullSpeed = 1,
    LowSpeed = 2,
    HighSpeed = 3,
    SuperSpeed = 4,
    SuperSpeedPlus = 5,
}

// ─── TRB (Transfer Request Block) ──────────────────────────────────────────

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Trb {
    pub parameter: u64,
    pub status: u32,
    pub control: u32,
}

impl Trb {
    pub fn new() -> Self {
        Self {
            parameter: 0,
            status: 0,
            control: 0,
        }
    }

    pub fn trb_type(&self) -> TrbType {
        TrbType::from_u8(((self.control >> 10) & 0x3F) as u8)
    }

    pub fn set_trb_type(&mut self, trb_type: TrbType) {
        self.control = (self.control & !(0x3F << 10)) | ((trb_type as u32) << 10);
    }

    pub fn cycle_bit(&self) -> bool {
        self.control & 1 != 0
    }

    pub fn set_cycle_bit(&mut self, cycle: bool) {
        if cycle {
            self.control |= 1;
        } else {
            self.control &= !1;
        }
    }

    pub fn toggle_cycle(&self) -> bool {
        self.control & (1 << 1) != 0
    }

    pub fn chain_bit(&self) -> bool {
        self.control & (1 << 4) != 0
    }

    pub fn set_chain_bit(&mut self, chain: bool) {
        if chain {
            self.control |= 1 << 4;
        } else {
            self.control &= !(1 << 4);
        }
    }

    pub fn ioc(&self) -> bool {
        self.control & (1 << 5) != 0
    }

    pub fn set_ioc(&mut self, ioc: bool) {
        if ioc {
            self.control |= 1 << 5;
        } else {
            self.control &= !(1 << 5);
        }
    }

    pub fn immediate_data(&self) -> bool {
        self.control & (1 << 6) != 0
    }

    pub fn set_immediate_data(&mut self, idt: bool) {
        if idt {
            self.control |= 1 << 6;
        } else {
            self.control &= !(1 << 6);
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrbType {
    Reserved = 0,
    Normal = 1,
    SetupStage = 2,
    DataStage = 3,
    StatusStage = 4,
    Isoch = 5,
    Link = 6,
    EventData = 7,
    NoOp = 8,
    EnableSlotCommand = 9,
    DisableSlotCommand = 10,
    AddressDeviceCommand = 11,
    ConfigureEndpointCommand = 12,
    EvaluateContextCommand = 13,
    ResetEndpointCommand = 14,
    StopEndpointCommand = 15,
    SetTrDequeuePointerCommand = 16,
    ResetDeviceCommand = 17,
    ForceEventCommand = 18,
    NegotiateBandwidthCommand = 19,
    SetLatencyToleranceCommand = 20,
    GetPortBandwidthCommand = 21,
    ForceHeaderCommand = 22,
    NoOpCommand = 23,
    GetExtendedPropertyCommand = 24,
    SetExtendedPropertyCommand = 25,
    TransferEvent = 32,
    CommandCompletionEvent = 33,
    PortStatusChangeEvent = 34,
    BandwidthRequestEvent = 35,
    DoorbellEvent = 36,
    HostControllerEvent = 37,
    DeviceNotificationEvent = 38,
    MfindexWrapEvent = 39,
}

impl TrbType {
    pub fn from_u8(val: u8) -> Self {
        match val {
            1 => TrbType::Normal,
            2 => TrbType::SetupStage,
            3 => TrbType::DataStage,
            4 => TrbType::StatusStage,
            5 => TrbType::Isoch,
            6 => TrbType::Link,
            7 => TrbType::EventData,
            8 => TrbType::NoOp,
            9 => TrbType::EnableSlotCommand,
            10 => TrbType::DisableSlotCommand,
            11 => TrbType::AddressDeviceCommand,
            12 => TrbType::ConfigureEndpointCommand,
            13 => TrbType::EvaluateContextCommand,
            14 => TrbType::ResetEndpointCommand,
            15 => TrbType::StopEndpointCommand,
            16 => TrbType::SetTrDequeuePointerCommand,
            17 => TrbType::ResetDeviceCommand,
            18 => TrbType::ForceEventCommand,
            19 => TrbType::NegotiateBandwidthCommand,
            20 => TrbType::SetLatencyToleranceCommand,
            21 => TrbType::GetPortBandwidthCommand,
            22 => TrbType::ForceHeaderCommand,
            23 => TrbType::NoOpCommand,
            24 => TrbType::GetExtendedPropertyCommand,
            25 => TrbType::SetExtendedPropertyCommand,
            32 => TrbType::TransferEvent,
            33 => TrbType::CommandCompletionEvent,
            34 => TrbType::PortStatusChangeEvent,
            35 => TrbType::BandwidthRequestEvent,
            36 => TrbType::DoorbellEvent,
            37 => TrbType::HostControllerEvent,
            38 => TrbType::DeviceNotificationEvent,
            39 => TrbType::MfindexWrapEvent,
            _ => TrbType::Reserved,
        }
    }

    pub fn is_transfer(&self) -> bool {
        matches!(
            self,
            TrbType::Normal
                | TrbType::SetupStage
                | TrbType::DataStage
                | TrbType::StatusStage
                | TrbType::Isoch
                | TrbType::EventData
                | TrbType::NoOp
        )
    }

    pub fn is_command(&self) -> bool {
        let v = *self as u8;
        v >= 9 && v <= 25
    }

    pub fn is_event(&self) -> bool {
        let v = *self as u8;
        v >= 32 && v <= 39
    }
}

// TRB completion codes
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrbCompletionCode {
    Invalid = 0,
    Success = 1,
    DataBufferError = 2,
    BabbleDetected = 3,
    UsbTransactionError = 4,
    TrbError = 5,
    StallError = 6,
    ResourceError = 7,
    BandwidthError = 8,
    NoSlotsAvailable = 9,
    InvalidStreamType = 10,
    SlotNotEnabled = 11,
    EndpointNotEnabled = 12,
    ShortPacket = 13,
    RingUnderrun = 14,
    RingOverrun = 15,
    VfEventRingFull = 16,
    ParameterError = 17,
    BandwidthOverrun = 18,
    ContextStateError = 19,
    NoPingResponse = 20,
    EventRingFull = 21,
    IncompatibleDevice = 22,
    MissedService = 23,
    CommandRingStopped = 24,
    CommandAborted = 25,
    Stopped = 26,
    StoppedLengthInvalid = 27,
    StoppedShortPacket = 28,
    MaxExitLatencyTooLarge = 29,
    IsochBufferOverrun = 31,
    EventLost = 32,
    Undefined = 33,
    InvalidStreamId = 34,
    SecondaryBandwidth = 35,
    SplitTransaction = 36,
}

impl TrbCompletionCode {
    pub fn from_u8(val: u8) -> Self {
        match val {
            1 => Self::Success,
            2 => Self::DataBufferError,
            3 => Self::BabbleDetected,
            4 => Self::UsbTransactionError,
            5 => Self::TrbError,
            6 => Self::StallError,
            7 => Self::ResourceError,
            8 => Self::BandwidthError,
            9 => Self::NoSlotsAvailable,
            10 => Self::InvalidStreamType,
            11 => Self::SlotNotEnabled,
            12 => Self::EndpointNotEnabled,
            13 => Self::ShortPacket,
            14 => Self::RingUnderrun,
            15 => Self::RingOverrun,
            16 => Self::VfEventRingFull,
            17 => Self::ParameterError,
            18 => Self::BandwidthOverrun,
            19 => Self::ContextStateError,
            20 => Self::NoPingResponse,
            21 => Self::EventRingFull,
            22 => Self::IncompatibleDevice,
            23 => Self::MissedService,
            24 => Self::CommandRingStopped,
            25 => Self::CommandAborted,
            26 => Self::Stopped,
            27 => Self::StoppedLengthInvalid,
            28 => Self::StoppedShortPacket,
            29 => Self::MaxExitLatencyTooLarge,
            31 => Self::IsochBufferOverrun,
            32 => Self::EventLost,
            34 => Self::InvalidStreamId,
            35 => Self::SecondaryBandwidth,
            36 => Self::SplitTransaction,
            _ => Self::Undefined,
        }
    }
}

// ─── Ring Management ────────────────────────────────────────────────────────

pub const RING_SEGMENT_SIZE: usize = 256;

/// Hypervisor CPUID leaf — vendor string at EBX/ECX/EDX when running under a VMM.
fn cpuid_hv_vendor_is_qemu() -> bool {
    let r1 = cpuid_leaf(1, 0);
    if r1.ecx & (1 << 31) == 0 {
        return false;
    }
    let hv = cpuid_leaf(0x4000_0000, 0);
    if hv.eax < 0x4000_0000 {
        return false;
    }
    let mut bytes = [0u8; 12];
    bytes[0..4].copy_from_slice(&hv.ebx.to_le_bytes());
    bytes[4..8].copy_from_slice(&hv.ecx.to_le_bytes());
    bytes[8..12].copy_from_slice(&hv.edx.to_le_bytes());
    let s = core::str::from_utf8(&bytes).unwrap_or("");
    s.starts_with("TCGTCGTCG") || s.contains("QEMU")
}

#[derive(Clone, Copy)]
struct CpuidRegs {
    eax: u32,
    ebx: u32,
    ecx: u32,
    edx: u32,
}

fn cpuid_leaf(leaf: u32, sub: u32) -> CpuidRegs {
    let mut r = CpuidRegs {
        eax: 0,
        ebx: 0,
        ecx: 0,
        edx: 0,
    };
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            tmp = lateout(reg) r.ebx,
            inout("eax") leaf => r.eax,
            inout("ecx") sub => r.ecx,
            lateout("edx") r.edx,
            options(nostack, preserves_flags),
        );
    }
    r
}

#[derive(Debug)]
pub struct TransferRing {
    pub page: DmaPage,
    pub enqueue_index: usize,
    pub dequeue_index: usize,
    pub cycle_state: bool,
    pub segment_size: usize,
}

impl TransferRing {
    pub fn new(size: usize) -> Result<Self, XhciError> {
        if size < 4 || size > 256 {
            return Err(XhciError::InvalidState);
        }
        let page = alloc_dma_page()?;
        let mut ring = Self {
            page,
            enqueue_index: 0,
            dequeue_index: 0,
            cycle_state: true,
            segment_size: size,
        };
        for i in 0..size {
            ring.trbs_mut()[i] = Trb::new();
        }
        let mut link_trb = Trb::new();
        link_trb.set_trb_type(TrbType::Link);
        link_trb.parameter = ring.phys_addr();
        link_trb.control |= 1 << 1; // toggle cycle
        ring.trbs_mut()[size - 1] = link_trb;
        Ok(ring)
    }

    fn trbs_mut(&mut self) -> &mut [Trb] {
        let count = self.segment_size;
        // Safety: one DMA page holds up to 256 TRBs (16 bytes each).
        unsafe { core::slice::from_raw_parts_mut(self.page.virt as *mut Trb, count) }
    }

    pub fn enqueue(&mut self, mut trb: Trb) -> Result<usize, XhciError> {
        if self.is_full() {
            return Err(XhciError::RingFull);
        }
        trb.set_cycle_bit(self.cycle_state);
        let idx = self.enqueue_index;
        self.trbs_mut()[idx] = trb;
        self.advance_enqueue();
        Ok(idx)
    }

    pub fn dequeue(&mut self) -> Option<Trb> {
        if self.is_empty() {
            return None;
        }
        let idx = self.dequeue_index;
        let trb = self.trbs_mut()[idx];
        self.dequeue_index = (idx + 1) % (self.segment_size - 1);
        Some(trb)
    }

    fn advance_enqueue(&mut self) {
        self.enqueue_index += 1;
        if self.enqueue_index >= self.segment_size - 1 {
            let link_idx = self.segment_size - 1;
            let cycle = self.cycle_state;
            self.trbs_mut()[link_idx].set_cycle_bit(cycle);
            self.cycle_state = !cycle;
            self.enqueue_index = 0;
        }
    }

    pub fn is_full(&self) -> bool {
        let next = (self.enqueue_index + 1) % (self.segment_size - 1);
        next == self.dequeue_index
    }

    pub fn is_empty(&self) -> bool {
        self.enqueue_index == self.dequeue_index
    }

    pub fn phys_addr(&self) -> u64 {
        self.page.phys
    }

    /// Zero the segment and rebuild the link TRB (fresh cycle for a new control transfer).
    pub fn clear_segment(&mut self) {
        let size = self.segment_size;
        for i in 0..size {
            self.trbs_mut()[i] = Trb::new();
        }
        let mut link_trb = Trb::new();
        link_trb.set_trb_type(TrbType::Link);
        link_trb.parameter = self.phys_addr();
        link_trb.control |= 1 << 1;
        self.trbs_mut()[size - 1] = link_trb;
        self.enqueue_index = 0;
        self.dequeue_index = 0;
        self.cycle_state = false;
    }
}

pub struct EventRing {
    pub trb_page: DmaPage,
    pub erst_page: DmaPage,
    pub dequeue_index: usize,
    pub cycle_state: bool,
    pub segment_size: usize,
}

impl EventRing {
    pub fn new(size: usize) -> Result<Self, XhciError> {
        let trb_page = alloc_dma_page()?;
        let erst_page = alloc_dma_page()?;
        let mut ring = Self {
            trb_page,
            erst_page,
            dequeue_index: 0,
            cycle_state: true,
            segment_size: size,
        };
        for i in 0..size {
            ring.trbs_mut()[i] = Trb::new();
        }
        let erst_entry = EventRingSegmentTableEntry {
            ring_segment_base_address: ring.trb_page.phys,
            ring_segment_size: size as u16,
            reserved: 0,
            reserved2: 0,
        };
        // Safety: ERST is a single entry in its own DMA page.
        unsafe {
            *(ring.erst_page.virt as *mut EventRingSegmentTableEntry) = erst_entry;
        }
        Ok(ring)
    }

    fn trbs_mut(&mut self) -> &mut [Trb] {
        // Safety: segment_size TRBs fit in one 4 KiB page.
        unsafe {
            core::slice::from_raw_parts_mut(self.trb_page.virt as *mut Trb, self.segment_size)
        }
    }

    pub fn has_pending_event(&self) -> bool {
        let trb = unsafe { &*(self.trb_page.virt as *const Trb).add(self.dequeue_index) };
        trb.cycle_bit() == self.cycle_state
    }

    pub fn dequeue_event(&mut self) -> Option<Trb> {
        if !self.has_pending_event() {
            return None;
        }
        let idx = self.dequeue_index;
        let trb = self.trbs_mut()[idx];
        self.dequeue_index = idx + 1;
        if self.dequeue_index >= self.segment_size {
            self.dequeue_index = 0;
            self.cycle_state = !self.cycle_state;
        }
        Some(trb)
    }

    pub fn dequeue_pointer(&self) -> u64 {
        self.trb_page.phys + (self.dequeue_index as u64) * core::mem::size_of::<Trb>() as u64
    }

    pub fn erst_phys_addr(&self) -> u64 {
        self.erst_page.phys
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct EventRingSegmentTableEntry {
    pub ring_segment_base_address: u64,
    pub ring_segment_size: u16,
    pub reserved: u16,
    pub reserved2: u32,
}

pub struct CommandRing {
    pub ring: TransferRing,
}

impl CommandRing {
    pub fn new() -> Result<Self, XhciError> {
        Ok(Self {
            ring: TransferRing::new(RING_SEGMENT_SIZE)?,
        })
    }

    pub fn enqueue_command(&mut self, trb: Trb) -> Result<usize, XhciError> {
        self.ring.enqueue(trb)
    }

    pub fn enable_slot(&mut self) -> Result<usize, XhciError> {
        let mut trb = Trb::new();
        trb.set_trb_type(TrbType::EnableSlotCommand);
        trb.set_ioc(true);
        self.enqueue_command(trb)
    }

    pub fn disable_slot(&mut self, slot_id: u8) -> Result<usize, XhciError> {
        let mut trb = Trb::new();
        trb.set_trb_type(TrbType::DisableSlotCommand);
        trb.control |= (slot_id as u32) << 24;
        self.enqueue_command(trb)
    }

    pub fn address_device(
        &mut self,
        slot_id: u8,
        input_context_ptr: u64,
        bsr: bool,
    ) -> Result<usize, XhciError> {
        let mut trb = Trb::new();
        trb.set_trb_type(TrbType::AddressDeviceCommand);
        trb.parameter = input_context_ptr;
        trb.control |= (slot_id as u32) << 24;
        if bsr {
            trb.control |= 1 << 9;
        }
        trb.set_ioc(true);
        self.enqueue_command(trb)
    }

    pub fn configure_endpoint(
        &mut self,
        slot_id: u8,
        input_context_ptr: u64,
        deconfigure: bool,
    ) -> Result<usize, XhciError> {
        let mut trb = Trb::new();
        trb.set_trb_type(TrbType::ConfigureEndpointCommand);
        trb.parameter = input_context_ptr;
        trb.control |= (slot_id as u32) << 24;
        if deconfigure {
            trb.control |= 1 << 9;
        }
        trb.set_ioc(true);
        self.enqueue_command(trb)
    }

    pub fn evaluate_context(
        &mut self,
        slot_id: u8,
        input_context_ptr: u64,
    ) -> Result<usize, XhciError> {
        let mut trb = Trb::new();
        trb.set_trb_type(TrbType::EvaluateContextCommand);
        trb.parameter = input_context_ptr;
        trb.control |= (slot_id as u32) << 24;
        self.enqueue_command(trb)
    }

    pub fn reset_endpoint(
        &mut self,
        slot_id: u8,
        endpoint_id: u8,
        tsp: bool,
    ) -> Result<usize, XhciError> {
        let mut trb = Trb::new();
        trb.set_trb_type(TrbType::ResetEndpointCommand);
        trb.control |= (slot_id as u32) << 24;
        trb.control |= (endpoint_id as u32) << 16;
        if tsp {
            trb.control |= 1 << 9;
        }
        trb.set_ioc(true);
        self.enqueue_command(trb)
    }

    pub fn stop_endpoint(
        &mut self,
        slot_id: u8,
        endpoint_id: u8,
        suspend: bool,
    ) -> Result<usize, XhciError> {
        let mut trb = Trb::new();
        trb.set_trb_type(TrbType::StopEndpointCommand);
        trb.control |= (slot_id as u32) << 24;
        trb.control |= (endpoint_id as u32) << 16;
        if suspend {
            trb.control |= 1 << 23;
        }
        trb.set_ioc(true);
        self.enqueue_command(trb)
    }

    pub fn set_tr_dequeue_pointer(
        &mut self,
        slot_id: u8,
        endpoint_id: u8,
        dequeue_ptr: u64,
        dcs: bool,
        stream_id: u16,
    ) -> Result<usize, XhciError> {
        let mut trb = Trb::new();
        trb.set_trb_type(TrbType::SetTrDequeuePointerCommand);
        trb.parameter = (dequeue_ptr & !0x0F) | (dcs as u64);
        trb.control |= (slot_id as u32) << 24;
        trb.control |= (endpoint_id as u32) << 16;
        trb.status = (stream_id as u32) << 16;
        trb.set_ioc(true);
        self.enqueue_command(trb)
    }

    pub fn reset_device(&mut self, slot_id: u8) -> Result<usize, XhciError> {
        let mut trb = Trb::new();
        trb.set_trb_type(TrbType::ResetDeviceCommand);
        trb.control |= (slot_id as u32) << 24;
        self.enqueue_command(trb)
    }

    pub fn force_event(
        &mut self,
        event_trb_ptr: u64,
        vf_interrupter: u16,
        vf_id: u8,
    ) -> Result<usize, XhciError> {
        let mut trb = Trb::new();
        trb.set_trb_type(TrbType::ForceEventCommand);
        trb.parameter = event_trb_ptr;
        trb.status = ((vf_id as u32) << 24) | ((vf_interrupter as u32) << 8);
        self.enqueue_command(trb)
    }

    pub fn noop(&mut self) -> Result<usize, XhciError> {
        let mut trb = Trb::new();
        trb.set_trb_type(TrbType::NoOpCommand);
        self.enqueue_command(trb)
    }

    pub fn phys_addr(&self) -> u64 {
        self.ring.phys_addr()
    }
}

// ─── Device Context ─────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SlotContext {
    pub data: [u32; 8],
}

impl SlotContext {
    pub fn route_string(&self) -> u32 {
        self.data[0] & 0xFFFFF
    }
    pub fn speed(&self) -> u8 {
        ((self.data[0] >> 20) & 0x0F) as u8
    }
    pub fn mtt(&self) -> bool {
        self.data[0] & (1 << 25) != 0
    }
    pub fn hub(&self) -> bool {
        self.data[0] & (1 << 26) != 0
    }
    pub fn context_entries(&self) -> u8 {
        ((self.data[0] >> 27) & 0x1F) as u8
    }

    pub fn max_exit_latency(&self) -> u16 {
        (self.data[1] & 0xFFFF) as u16
    }
    pub fn root_hub_port_number(&self) -> u8 {
        ((self.data[1] >> 16) & 0xFF) as u8
    }
    pub fn num_ports(&self) -> u8 {
        ((self.data[1] >> 24) & 0xFF) as u8
    }

    pub fn parent_hub_slot_id(&self) -> u8 {
        (self.data[2] & 0xFF) as u8
    }
    pub fn parent_port_number(&self) -> u8 {
        ((self.data[2] >> 8) & 0xFF) as u8
    }
    pub fn tt_think_time(&self) -> u8 {
        ((self.data[2] >> 16) & 0x03) as u8
    }
    pub fn interrupter_target(&self) -> u16 {
        ((self.data[2] >> 22) & 0x3FF) as u16
    }

    pub fn usb_device_address(&self) -> u8 {
        (self.data[3] & 0xFF) as u8
    }
    pub fn slot_state(&self) -> SlotState {
        match (self.data[3] >> 27) & 0x1F {
            0 => SlotState::DisabledEnabled,
            1 => SlotState::Default,
            2 => SlotState::Addressed,
            3 => SlotState::Configured,
            _ => SlotState::DisabledEnabled,
        }
    }

    pub fn set_route_string(&mut self, route: u32) {
        self.data[0] = (self.data[0] & !0xFFFFF) | (route & 0xFFFFF);
    }

    pub fn set_speed(&mut self, speed: u8) {
        self.data[0] = (self.data[0] & !(0x0F << 20)) | ((speed as u32 & 0x0F) << 20);
    }

    pub fn set_context_entries(&mut self, entries: u8) {
        self.data[0] = (self.data[0] & !(0x1F << 27)) | (((entries & 0x1F) as u32) << 27);
    }

    pub fn set_root_hub_port(&mut self, port: u8) {
        self.data[1] = (self.data[1] & !(0xFF << 16)) | ((port as u32) << 16);
    }

    pub fn set_interrupter_target(&mut self, target: u16) {
        self.data[2] = (self.data[2] & !(0x3FF << 22)) | (((target & 0x3FF) as u32) << 22);
    }

    /// Set/clear the Hub bit (data[0] bit 26). Required so the xHC routes
    /// downstream traffic and split transactions through this slot's TT.
    pub fn set_hub(&mut self, is_hub: bool) {
        if is_hub {
            self.data[0] |= 1 << 26;
        } else {
            self.data[0] &= !(1 << 26);
        }
    }

    /// Multi-TT bit (data[0] bit 25) — set when a high-speed hub exposes one
    /// transaction translator per downstream port.
    pub fn set_mtt(&mut self, multi_tt: bool) {
        if multi_tt {
            self.data[0] |= 1 << 25;
        } else {
            self.data[0] &= !(1 << 25);
        }
    }

    /// Number of downstream ports (data[1] bits 31:24) — only meaningful when
    /// the Hub bit is set.
    pub fn set_num_ports(&mut self, ports: u8) {
        self.data[1] = (self.data[1] & !(0xFF << 24)) | ((ports as u32) << 24);
    }

    /// Parent hub slot id (data[2] bits 7:0) — the slot of the hub this device
    /// is attached to. Zero for root-port devices.
    pub fn set_parent_hub_slot(&mut self, slot: u8) {
        self.data[2] = (self.data[2] & !0xFF) | (slot as u32);
    }

    /// Parent port number (data[2] bits 15:8) — the downstream port on the
    /// parent hub this device hangs off.
    pub fn set_parent_port(&mut self, port: u8) {
        self.data[2] = (self.data[2] & !(0xFF << 8)) | ((port as u32) << 8);
    }

    /// TT think time (data[2] bits 17:16) — 0..3 → 8/16/24/32 FS bit times.
    pub fn set_tt_think_time(&mut self, ttt: u8) {
        self.data[2] = (self.data[2] & !(0x03 << 16)) | (((ttt & 0x03) as u32) << 16);
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotState {
    DisabledEnabled = 0,
    Default = 1,
    Addressed = 2,
    Configured = 3,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct EndpointContext {
    pub data: [u32; 8],
}

impl EndpointContext {
    pub fn ep_state(&self) -> EndpointState {
        match self.data[0] & 0x07 {
            0 => EndpointState::Disabled,
            1 => EndpointState::Running,
            2 => EndpointState::Halted,
            3 => EndpointState::Stopped,
            4 => EndpointState::Error,
            _ => EndpointState::Disabled,
        }
    }

    pub fn mult(&self) -> u8 {
        ((self.data[0] >> 8) & 0x03) as u8
    }
    pub fn max_primary_streams(&self) -> u8 {
        ((self.data[0] >> 10) & 0x1F) as u8
    }
    pub fn linear_stream_array(&self) -> bool {
        self.data[0] & (1 << 15) != 0
    }
    pub fn interval(&self) -> u8 {
        ((self.data[0] >> 16) & 0xFF) as u8
    }
    pub fn max_esit_payload_hi(&self) -> u8 {
        ((self.data[0] >> 24) & 0xFF) as u8
    }

    pub fn error_count(&self) -> u8 {
        ((self.data[1] >> 1) & 0x03) as u8
    }
    pub fn ep_type(&self) -> EndpointType {
        match (self.data[1] >> 3) & 0x07 {
            0 => EndpointType::NotValid,
            1 => EndpointType::IsochOut,
            2 => EndpointType::BulkOut,
            3 => EndpointType::InterruptOut,
            4 => EndpointType::Control,
            5 => EndpointType::IsochIn,
            6 => EndpointType::BulkIn,
            7 => EndpointType::InterruptIn,
            _ => EndpointType::NotValid,
        }
    }

    pub fn max_burst_size(&self) -> u8 {
        ((self.data[1] >> 8) & 0xFF) as u8
    }
    pub fn max_packet_size(&self) -> u16 {
        ((self.data[1] >> 16) & 0xFFFF) as u16
    }

    pub fn tr_dequeue_pointer(&self) -> u64 {
        ((self.data[2] as u64) | ((self.data[3] as u64) << 32)) & !0x0F
    }

    pub fn dequeue_cycle_state(&self) -> bool {
        self.data[2] & 1 != 0
    }

    pub fn average_trb_length(&self) -> u16 {
        (self.data[4] & 0xFFFF) as u16
    }
    pub fn max_esit_payload_lo(&self) -> u16 {
        ((self.data[4] >> 16) & 0xFFFF) as u16
    }

    pub fn set_ep_type(&mut self, ep_type: EndpointType) {
        self.data[1] = (self.data[1] & !(0x07 << 3)) | ((ep_type as u32) << 3);
    }

    pub fn set_max_packet_size(&mut self, mps: u16) {
        self.data[1] = (self.data[1] & !0xFFFF_0000) | ((mps as u32) << 16);
    }

    pub fn set_max_burst_size(&mut self, burst: u8) {
        self.data[1] = (self.data[1] & !(0xFF << 8)) | ((burst as u32) << 8);
    }

    pub fn set_tr_dequeue_pointer(&mut self, ptr: u64, dcs: bool) {
        let val = (ptr & !0x0F) | (dcs as u64);
        self.data[2] = val as u32;
        self.data[3] = (val >> 32) as u32;
    }

    pub fn set_interval(&mut self, interval: u8) {
        self.data[0] = (self.data[0] & !(0xFF << 16)) | ((interval as u32) << 16);
    }

    pub fn set_error_count(&mut self, count: u8) {
        self.data[1] = (self.data[1] & !(0x03 << 1)) | (((count & 0x03) as u32) << 1);
    }

    pub fn set_average_trb_length(&mut self, len: u16) {
        self.data[4] = (self.data[4] & !0xFFFF) | (len as u32);
    }

    /// Max ESIT Payload (low 16 bits, dword 4 bits 31:16) — the maximum bytes
    /// the endpoint moves per Endpoint Service Interval. Required for isochronous
    /// (and high-bandwidth interrupt) endpoints so the xHC reserves bus bandwidth.
    pub fn set_max_esit_payload_lo(&mut self, payload: u16) {
        self.data[4] = (self.data[4] & 0x0000_FFFF) | ((payload as u32) << 16);
    }

    /// Mult (dword 0 bits 9:8) — number of bursts per ESIT minus one. 0 for
    /// USB2 isoch / single-burst SuperSpeed.
    pub fn set_mult(&mut self, mult: u8) {
        self.data[0] = (self.data[0] & !(0x03 << 8)) | (((mult & 0x03) as u32) << 8);
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointState {
    Disabled = 0,
    Running = 1,
    Halted = 2,
    Stopped = 3,
    Error = 4,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointType {
    NotValid = 0,
    IsochOut = 1,
    BulkOut = 2,
    InterruptOut = 3,
    Control = 4,
    IsochIn = 5,
    BulkIn = 6,
    InterruptIn = 7,
}

impl EndpointType {
    pub fn from_ep_address(address: u8, transfer_type: u8) -> Self {
        let dir_in = address & 0x80 != 0;
        match (transfer_type & 0x03, dir_in) {
            (0, _) => EndpointType::Control,
            (1, false) => EndpointType::IsochOut,
            (1, true) => EndpointType::IsochIn,
            (2, false) => EndpointType::BulkOut,
            (2, true) => EndpointType::BulkIn,
            (3, false) => EndpointType::InterruptOut,
            (3, true) => EndpointType::InterruptIn,
            _ => EndpointType::NotValid,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct DeviceContext {
    pub slot: SlotContext,
    pub endpoints: [EndpointContext; 31],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct InputControlContext {
    pub drop_flags: u32,
    pub add_flags: u32,
    pub reserved: [u32; 5],
    pub configuration_value: u8,
    pub interface_number: u8,
    pub alternate_setting: u8,
    pub reserved2: u8,
}

impl InputControlContext {
    /// `ep_index` is the [`DeviceContext::endpoints`] array index; sets `A{ep_index+1}`.
    pub fn add_endpoint(&mut self, ep_index: u8) {
        self.add_flags |= 1 << (ep_index + 1);
    }

    pub fn drop_endpoint(&mut self, ep_index: u8) {
        self.drop_flags |= 1 << (ep_index + 1);
    }

    pub fn add_slot_context(&mut self) {
        self.add_flags |= 1;
    }

    pub fn has_add_endpoint(&self, ep_index: u8) -> bool {
        self.add_flags & (1 << (ep_index + 1)) != 0
    }
}

/// Compact in-memory layout for **Address Device** and **Configure Endpoint**:
/// ICC @0 (32 B), device @ +32. CSZ=1 controllers consume a 64-byte-stride image,
/// produced from this compact layout by `expand_input_context_csz64` right before
/// the command is issued (NOT by repacking the device to +64 — see the removed
/// spec-layout path in `configure_hid_interrupt`).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct InputContext {
    pub control: InputControlContext,
    pub device: DeviceContext,
}

// Device-context byte offsets within the input-context page. `_SPEC` (+64) is the
// xHCI §6.2.5 spec stride, still referenced by the diagnostic dump's spec branch.
const INPUT_CTX_DEVICE_OFFSET_COMPACT: usize = 32;
const INPUT_CTX_DEVICE_OFFSET_SPEC: usize = 64;

// ─── DCBAA (Device Context Base Address Array) ──────────────────────────────

pub struct Dcbaa {
    pub page: DmaPage,
    pub max_slots: u8,
}

impl Dcbaa {
    pub fn new(max_slots: u8) -> Result<Self, XhciError> {
        Ok(Self {
            page: alloc_dma_page()?,
            max_slots,
        })
    }

    fn entries_mut(&mut self) -> &mut [u64] {
        let count = (self.max_slots as usize) + 1;
        // Safety: DCBAA needs (MaxSlots+1) pointers; fits in one 4 KiB page.
        unsafe { core::slice::from_raw_parts_mut(self.page.virt as *mut u64, count) }
    }

    pub fn set_device_context(&mut self, slot_id: u8, context_ptr: u64) {
        if slot_id <= self.max_slots {
            self.entries_mut()[slot_id as usize] = context_ptr;
        }
    }

    pub fn get_device_context(&self, slot_id: u8) -> u64 {
        if slot_id <= self.max_slots {
            let count = (self.max_slots as usize) + 1;
            let entries =
                unsafe { core::slice::from_raw_parts(self.page.virt as *const u64, count) };
            entries[slot_id as usize]
        } else {
            0
        }
    }

    pub fn phys_addr(&self) -> u64 {
        self.page.phys
    }
}

// ─── Scratchpad Buffers ─────────────────────────────────────────────────────

pub struct ScratchpadBuffers {
    pub array_page: DmaPage,
    pub buffers: Vec<DmaPage>,
    pub page_size: usize,
}

impl ScratchpadBuffers {
    pub fn new(count: u32, page_size: usize) -> Result<Self, XhciError> {
        let array_page = alloc_dma_page()?;
        let mut buffers = Vec::with_capacity(count as usize);
        for i in 0..count {
            let buf = alloc_dma_page()?;
            // Safety: scratchpad pointer table lives in array_page.
            unsafe {
                *((array_page.virt as *mut u64).add(i as usize)) = buf.phys;
            }
            buffers.push(buf);
        }
        Ok(Self {
            array_page,
            buffers,
            page_size,
        })
    }

    pub fn array_phys_addr(&self) -> u64 {
        self.array_page.phys
    }
}

// ─── Interrupter Registers ──────────────────────────────────────────────────

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct InterrupterRegisters {
    pub iman: u32,
    pub imod: u32,
    pub erstsz: u32,
    pub reserved: u32,
    pub erstba: u64,
    pub erdp: u64,
}

pub const IMAN_IP: u32 = 1 << 0;
pub const IMAN_IE: u32 = 1 << 1;

impl InterrupterRegisters {
    pub fn interrupt_pending(&self) -> bool {
        self.iman & IMAN_IP != 0
    }
    pub fn interrupt_enabled(&self) -> bool {
        self.iman & IMAN_IE != 0
    }

    pub fn moderation_interval(&self) -> u16 {
        (self.imod & 0xFFFF) as u16
    }
    pub fn moderation_counter(&self) -> u16 {
        ((self.imod >> 16) & 0xFFFF) as u16
    }
}

// ─── Extended Capabilities ──────────────────────────────────────────────────

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtendedCapabilityId {
    Reserved = 0,
    UsbLegacySupport = 1,
    SupportedProtocol = 2,
    ExtendedPowerManagement = 3,
    IoVirtualization = 4,
    MessageInterrupt = 5,
    LocalMemory = 6,
    UsbDebugCapability = 10,
    ExtendedMessageInterrupt = 17,
}

#[derive(Debug, Clone, Copy)]
pub struct UsbLegacySupportCap {
    pub cap_id: u8,
    pub next_ptr: u8,
    pub bios_owned: bool,
    pub os_owned: bool,
}

#[derive(Debug, Clone)]
pub struct SupportedProtocolCap {
    pub cap_id: u8,
    pub next_ptr: u8,
    pub revision_minor: u8,
    pub revision_major: u8,
    pub name: [u8; 4],
    pub compatible_port_offset: u8,
    pub compatible_port_count: u8,
    pub protocol_defined: u16,
    pub protocol_slot_type: u8,
    pub protocol_speed_ids: Vec<ProtocolSpeedId>,
}

#[derive(Debug, Clone, Copy)]
pub struct ProtocolSpeedId {
    pub value: u32,
}

impl ProtocolSpeedId {
    pub fn speed_id_value(&self) -> u8 {
        (self.value & 0x0F) as u8
    }
    pub fn speed_id_exponent(&self) -> u8 {
        ((self.value >> 4) & 0x03) as u8
    }
    pub fn psi_type(&self) -> u8 {
        ((self.value >> 6) & 0x03) as u8
    }
    pub fn full_duplex(&self) -> bool {
        self.value & (1 << 8) != 0
    }
    pub fn link_protocol(&self) -> u8 {
        ((self.value >> 14) & 0x03) as u8
    }
    pub fn speed_mantissa(&self) -> u16 {
        ((self.value >> 16) & 0xFFFF) as u16
    }

    pub fn speed_bps(&self) -> u64 {
        let mantissa = self.speed_mantissa() as u64;
        match self.speed_id_exponent() {
            0 => mantissa,
            1 => mantissa * 1_000,
            2 => mantissa * 1_000_000,
            3 => mantissa * 1_000_000_000,
            _ => 0,
        }
    }
}

// ─── Doorbell Register ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct DoorbellRegister {
    pub value: u32,
}

impl DoorbellRegister {
    pub fn new(target: u8, stream_id: u16) -> Self {
        Self {
            value: (target as u32) | ((stream_id as u32) << 16),
        }
    }

    pub fn host_controller() -> Self {
        Self { value: 0 }
    }

    pub fn target(&self) -> u8 {
        (self.value & 0xFF) as u8
    }
    pub fn stream_id(&self) -> u16 {
        ((self.value >> 16) & 0xFFFF) as u16
    }
}

// ─── Device Slot Management ─────────────────────────────────────────────────

#[derive(Debug)]
pub struct DeviceSlot {
    pub slot_id: u8,
    pub output_ctx: DmaPage,
    pub input_ctx: DmaPage,
    pub transfer_rings: Vec<Option<TransferRing>>,
    pub port_id: u8,
    pub speed: PortSpeed,
    pub enabled: bool,
    pub addressed: bool,
    pub configured: bool,
    pub max_packet_size_ep0: u16,
    /// Every bound HID interrupt-IN interface on this device. A device may
    /// expose several — composite gaming peripherals pack a mouse + a
    /// keyboard/media interface into one device — so we bind them all, not just
    /// the first (binding only the first missed the Razer 1532:0098's pointer
    /// interface → dead cursor). Empty for non-HID devices.
    pub hid_interfaces: Vec<HidInterface>,
}

/// Number of interrupt-IN TDs kept outstanding per HID endpoint (multi-buffering).
/// The endpoint is drained by a kernel thread that, on the single post-boot
/// scheduling CPU, is only PICKED ~8×/s. With a SINGLE outstanding TD the xHC can
/// buffer just one report between drains, so a 125–1000 Hz mouse overflowed and
/// only ~8 position updates/s reached the cursor → it crawled (iron). Queuing a
/// ring of TDs lets the controller buffer a BATCH the thread pulls each cycle
/// (~8 cycles/s × 16 ≈ 128 reports/s) — a responsive cursor without busy-spinning.
const HID_TD_RING: u64 = 16;
/// Per-report slot stride in the (4 KiB) report buffer. 256 B/slot × 16 slots =
/// 4096 B = exactly one page, and 256 ≥ any HID interrupt report (boot ≤8 B,
/// report-protocol ≤64 B), so a TD never writes past its slot into the next.
const HID_SLOT_SIZE: u64 = 256;

/// One bound HID interrupt-IN interface. A composite device contributes several
/// of these to a single [`DeviceSlot`].
#[derive(Debug)]
pub struct HidInterface {
    pub ep: HidInterruptEndpoint,
    pub report_buf: DmaPage,
    pub input_device_id: u64,
    /// Report-descriptor decoder for NON-boot-protocol interfaces (gaming
    /// devices that don't implement boot protocol). `Some` only in report
    /// protocol; boot interfaces keep this `None` and use the fixed boot-layout
    /// parser. Built via `raehid` (wraps Redox's `hidreport`).
    pub hid_device: Option<raehid::HidDevice>,
    /// Round-robin index into the `HID_TD_RING` report slots. The xHC completes a
    /// transfer ring's TDs IN ORDER, so completion N drains slot `N % HID_TD_RING`
    /// and re-arms a TD into that same slot — keeping `HID_TD_RING` outstanding.
    pub drain_seq: u64,
    /// Best-effort count of interrupt-IN TDs currently queued on this endpoint's
    /// transfer ring (incremented on submit, decremented on completion). The
    /// drain loop tops this back up to `HID_TD_RING` every service cycle: if the
    /// controller ever drains the ring to empty (a burst of reports + a missed
    /// re-arm, or a controller that stops the EP on an empty ring), outstanding
    /// hits 0 and NO further completion event can ever be posted to re-arm from
    /// — the SILENT wedge (no error logged) the beta-tester hit after ~16 events.
    /// Topping up unconditionally each cycle makes HID flow INDEFINITELY.
    pub outstanding_tds: u64,
}

impl DeviceSlot {
    pub fn new(slot_id: u8, port_id: u8, speed: PortSpeed) -> Result<Self, XhciError> {
        let max_packet_size_ep0 = match speed {
            PortSpeed::LowSpeed => 8,
            PortSpeed::FullSpeed => 8,
            PortSpeed::HighSpeed => 64,
            PortSpeed::SuperSpeed | PortSpeed::SuperSpeedPlus => 512,
            PortSpeed::Undefined => 8,
        };
        let mut transfer_rings: Vec<Option<TransferRing>> = Vec::with_capacity(31);
        for _ in 0..31 {
            transfer_rings.push(None);
        }
        let mut ep0_ring = TransferRing::new(RING_SEGMENT_SIZE)?;
        ep0_ring.cycle_state = false;
        transfer_rings[0] = Some(ep0_ring);
        Ok(Self {
            slot_id,
            output_ctx: alloc_dma_page()?,
            input_ctx: alloc_dma_page()?,
            transfer_rings,
            port_id,
            speed,
            enabled: true,
            addressed: false,
            configured: false,
            max_packet_size_ep0,
            hid_interfaces: Vec::new(),
        })
    }

    pub fn prepare_configure_hid(
        &mut self,
        config_value: u8,
        ep: HidInterruptEndpoint,
    ) -> Result<(), XhciError> {
        let out = unsafe { &*(self.output_ctx.virt as *const DeviceContext) };

        // Zero the whole input-context page before rebuilding it, so no stale
        // bytes from a prior command (e.g. the 64-byte-stride image left by a
        // previous Address Device / Configure Endpoint on a CSZ=1 controller)
        // survive in fields this command does not explicitly overwrite. Defensive
        // — only Add-flagged contexts are read by the xHC and those are fully
        // written below, but this matches prepare_evaluate_ep0_mps and removes a
        // class of latent fragility under expand_input_context_csz64.
        unsafe {
            core::ptr::write_bytes(self.input_ctx.virt as *mut u8, 0, 4096);
        }
        self.input_context().control = InputControlContext::default();
        // Slot + new interrupt endpoint only (EP0 left as-is in output context).
        self.input_context().control.add_slot_context();
        self.input_context().control.add_endpoint(ep.ep_index);
        self.input_context().control.configuration_value = config_value;
        self.input_context().control.interface_number = ep.interface_number;
        self.input_context().control.alternate_setting = ep.alternate_setting;

        self.input_context().device.slot = out.slot;
        let last_ctx_index = ep.ep_index + 1;
        self.input_context()
            .device
            .slot
            .set_context_entries(last_ctx_index);

        let idx = ep.ep_index as usize;
        if idx >= 31 {
            return Err(XhciError::InvalidEndpoint);
        }

        // xHCI §6.2.3.6: the Endpoint Context Interval field is an EXPONENT
        // (period = 2^Interval × 125 µs) — NOT the descriptor's raw bInterval.
        //   * LS/FS interrupt: bInterval is in 1 ms frames; legal Interval is
        //     3..=10 (2^3 × 125 µs = 1 ms). Writing the raw value (e.g. the
        //     Razer FS keyboard's bInterval=1 → 250 µs) is out of range for
        //     the speed: QEMU accepts it, AMD's real xHC rejects the whole
        //     Configure Endpoint with ParameterError — the photographed
        //     Athena HID failure.
        //   * HS/SS interrupt: bInterval IS already exponent+1 in 125 µs
        //     units; Interval = bInterval − 1, clamped to 0..=15.
        let interval = match self.speed {
            PortSpeed::LowSpeed | PortSpeed::FullSpeed => {
                let frames_125us = (ep.interval.max(1) as u32) * 8;
                let mut i = 3u8;
                while i < 10 && (1u32 << i) < frames_125us {
                    i += 1;
                }
                i
            }
            _ => ep.interval.clamp(1, 16) - 1,
        };

        if self.transfer_rings[idx].is_none() {
            let mut ring = TransferRing::new(RING_SEGMENT_SIZE)?;
            ring.cycle_state = true;
            self.transfer_rings[idx] = Some(ring);
        }

        let ring_phys = self.transfer_rings[idx]
            .as_ref()
            .map(|r| r.phys_addr())
            .ok_or(XhciError::EndpointNotConfigured)?;

        let ep_ctx = &mut self.input_context().device.endpoints[idx];
        ep_ctx.set_ep_type(EndpointType::InterruptIn);
        ep_ctx.set_max_packet_size(ep.max_packet_size);
        ep_ctx.set_max_burst_size(0);
        ep_ctx.set_interval(interval);
        ep_ctx.set_error_count(3);
        ep_ctx.set_average_trb_length(ep.max_packet_size);
        // xHCI §6.2.3.8: Max ESIT Payload = Max Packet Size × (Max Burst + 1).
        // REQUIRED for interrupt/isoch endpoints — left at 0 (as it was), AMD's
        // strict xHC rejects the whole Configure Endpoint with ParameterError
        // (the photographed Athena keyboard failure: Razer 1532:0098, FS,
        // mps=8). QEMU tolerates a 0 ESIT, which is why it passed there. Burst
        // is 0 here, so ESIT = max_packet_size; hi bits stay 0 (mps ≤ 1024).
        ep_ctx.set_max_esit_payload_lo(ep.max_packet_size);
        // New rings: DCS=1 per xHCI §4.9 (first TRB cycle bit matches DCS).
        ep_ctx.set_tr_dequeue_pointer(ring_phys, true);

        // Endpoint state is tracked per-interface in DeviceSlot::hid_interfaces
        // by the bring-up caller (multi-interface aware), not as a single field.
        Ok(())
    }

    /// Build an input context that adds BOTH bulk endpoints (IN + OUT) of an
    /// MSC Bulk-Only-Transport interface, each with a fresh transfer ring.
    /// Issued via Configure Endpoint, mirroring [`Self::prepare_configure_hid`]
    /// but for two bulk endpoints instead of one interrupt-IN.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_configure_msc_bulk(
        &mut self,
        config_value: u8,
        interface_number: u8,
        in_ep_index: u8,
        in_mps: u16,
        in_burst: u8,
        out_ep_index: u8,
        out_mps: u16,
        out_burst: u8,
    ) -> Result<(), XhciError> {
        let out = unsafe { &*(self.output_ctx.virt as *const DeviceContext) };

        // Zero the whole input-context page before rebuilding it, so no stale
        // bytes from a prior command (e.g. the 64-byte-stride image left by a
        // previous Address Device / Configure Endpoint on a CSZ=1 controller)
        // survive in fields this command does not explicitly overwrite. Defensive
        // — only Add-flagged contexts are read by the xHC and those are fully
        // written below, but this matches prepare_evaluate_ep0_mps and removes a
        // class of latent fragility under expand_input_context_csz64.
        unsafe {
            core::ptr::write_bytes(self.input_ctx.virt as *mut u8, 0, 4096);
        }
        self.input_context().control = InputControlContext::default();
        self.input_context().control.add_slot_context();
        self.input_context().control.add_endpoint(in_ep_index);
        self.input_context().control.add_endpoint(out_ep_index);
        self.input_context().control.configuration_value = config_value;
        self.input_context().control.interface_number = interface_number;
        self.input_context().control.alternate_setting = 0;

        self.input_context().device.slot = out.slot;
        let last_ctx_index = in_ep_index.max(out_ep_index) + 1;
        self.input_context()
            .device
            .slot
            .set_context_entries(last_ctx_index);

        // Configure each bulk endpoint: fresh ring + endpoint context.
        // SuperSpeed bulk endpoints carry a Max Burst Size from their SS EP
        // Companion descriptor; USB2 endpoints report burst 0. Setting the
        // burst lets the xHC issue multi-packet bursts on SS+ links — the
        // throughput distinction from the USB2 path (xHCI §4.8.2 / §6.2.3.4).
        for &(idx_u8, mps, burst, is_in) in &[
            (in_ep_index, in_mps, in_burst, true),
            (out_ep_index, out_mps, out_burst, false),
        ] {
            let idx = idx_u8 as usize;
            if idx >= 31 {
                return Err(XhciError::InvalidEndpoint);
            }
            if self.transfer_rings[idx].is_none() {
                let mut ring = TransferRing::new(RING_SEGMENT_SIZE)?;
                ring.cycle_state = true;
                self.transfer_rings[idx] = Some(ring);
            }
            let ring_phys = self.transfer_rings[idx]
                .as_ref()
                .map(|r| r.phys_addr())
                .ok_or(XhciError::EndpointNotConfigured)?;
            let ep_ctx = &mut self.input_context().device.endpoints[idx];
            ep_ctx.set_ep_type(if is_in {
                EndpointType::BulkIn
            } else {
                EndpointType::BulkOut
            });
            ep_ctx.set_max_packet_size(mps);
            ep_ctx.set_max_burst_size(burst);
            ep_ctx.set_interval(0); // bulk endpoints have no polling interval
            ep_ctx.set_error_count(3);
            ep_ctx.set_average_trb_length(mps.max(512));
            ep_ctx.set_tr_dequeue_pointer(ring_phys, true);
        }
        Ok(())
    }

    /// Build an input context configuring ONE isochronous OUT endpoint for a USB
    /// Audio Class streaming interface (Configure Endpoint, xHCI §4.3.5 / §6.2.3).
    /// `interval` is the xHCI-encoded service interval (FS audio: 3 = 1 ms);
    /// `mps` is the max packet / per-ESIT payload (FS 48k/16/stereo ≈ 192 B).
    pub fn prepare_configure_isoch_out(
        &mut self,
        config_value: u8,
        interface_number: u8,
        alt_setting: u8,
        ep_index: u8,
        mps: u16,
        interval: u8,
    ) -> Result<(), XhciError> {
        let idx = ep_index as usize;
        if idx >= 31 {
            return Err(XhciError::InvalidEndpoint);
        }
        let out = unsafe { &*(self.output_ctx.virt as *const DeviceContext) };

        // Zero the whole input-context page before rebuilding it, so no stale
        // bytes from a prior command (e.g. the 64-byte-stride image left by a
        // previous Address Device / Configure Endpoint on a CSZ=1 controller)
        // survive in fields this command does not explicitly overwrite. Defensive
        // — only Add-flagged contexts are read by the xHC and those are fully
        // written below, but this matches prepare_evaluate_ep0_mps and removes a
        // class of latent fragility under expand_input_context_csz64.
        unsafe {
            core::ptr::write_bytes(self.input_ctx.virt as *mut u8, 0, 4096);
        }
        self.input_context().control = InputControlContext::default();
        self.input_context().control.add_slot_context();
        self.input_context().control.add_endpoint(ep_index);
        self.input_context().control.configuration_value = config_value;
        self.input_context().control.interface_number = interface_number;
        self.input_context().control.alternate_setting = alt_setting;

        self.input_context().device.slot = out.slot;
        self.input_context()
            .device
            .slot
            .set_context_entries(ep_index + 1);

        if self.transfer_rings[idx].is_none() {
            let mut ring = TransferRing::new(RING_SEGMENT_SIZE)?;
            ring.cycle_state = true;
            self.transfer_rings[idx] = Some(ring);
        }
        let ring_phys = self.transfer_rings[idx]
            .as_ref()
            .map(|r| r.phys_addr())
            .ok_or(XhciError::EndpointNotConfigured)?;
        let ep_ctx = &mut self.input_context().device.endpoints[idx];
        ep_ctx.set_ep_type(EndpointType::IsochOut);
        ep_ctx.set_max_packet_size(mps);
        ep_ctx.set_max_burst_size(0); // USB2 full-speed: single packet per burst
        ep_ctx.set_mult(0);
        ep_ctx.set_interval(interval);
        ep_ctx.set_error_count(0); // isoch endpoints do not retry (CErr = 0)
        ep_ctx.set_average_trb_length(mps);
        ep_ctx.set_max_esit_payload_lo(mps);
        ep_ctx.set_tr_dequeue_pointer(ring_phys, true);
        Ok(())
    }

    /// Build an input context that updates only the slot context to mark this
    /// device as a hub (Hub bit, port count, TT params). Issued via a Configure
    /// Endpoint command — xHCI §4.6.6 applies slot-context fields when A0 is set.
    /// Compact layout (ICC @0, device @ +32) to match QEMU, same as Address Device.
    pub fn prepare_configure_hub(&mut self, num_ports: u8, ttt: u8, multi_tt: bool) {
        let out = unsafe { &*(self.output_ctx.virt as *const DeviceContext) };
        let mut slot_ctx = out.slot;
        slot_ctx.set_hub(true);
        slot_ctx.set_num_ports(num_ports);
        slot_ctx.set_tt_think_time(ttt);
        slot_ctx.set_mtt(multi_tt);

        // Zero the input-context page first (see prepare_configure_hid) so no
        // stale bytes from a prior command survive into this Configure Endpoint.
        unsafe {
            core::ptr::write_bytes(self.input_ctx.virt as *mut u8, 0, 4096);
        }
        self.input_context().control = InputControlContext::default();
        self.input_context().control.add_slot_context();
        self.input_context().device.slot = slot_ctx;
    }

    /// CSZ=1 fix (Athena photo #3: every Address Device -> ParameterError).
    /// The driver builds the input context compact — 32-byte entries: ICC@0,
    /// slot@32, EP(DCI d)@32*(d+1) — which is the correct layout for CSZ=0
    /// controllers (QEMU). A CSZ=1 controller (HCCPARAMS1 bit 2, e.g.
    /// Athena's AMD 1022:15b9) reads 64-byte entries, so the compact image
    /// is garbage to it. This rewrites the page in place to 64-byte stride:
    /// entry k moves 32k -> 64k (backwards, so sources are read before
    /// they're overwritten), upper 32 bytes of each entry zeroed. Call after
    /// building the compact context, immediately before issuing the command.
    pub fn expand_input_context_csz64(&self) {
        // 33 entries: ICC (k=0), slot (k=1), EP DCI 1..=31 (k=2..=32).
        // 33 * 64 = 2112 bytes — fits the 4 KiB input_ctx page.
        unsafe {
            let base = self.input_ctx.virt as *mut u8;
            for k in (1..33usize).rev() {
                core::ptr::copy(base.add(32 * k), base.add(64 * k), 32);
                core::ptr::write_bytes(base.add(64 * k + 32), 0, 32);
            }
            // ICC stays at 0; zero its upper half (slot already moved out).
            core::ptr::write_bytes(base.add(32), 0, 32);
        }
    }

    /// Stride-aware read of one endpoint context from the OUTPUT device
    /// context the xHC writes (entry index = DCI; the slot context is entry
    /// 0). With CSZ=1 entries are 64 bytes apart — indexing the packed
    /// `DeviceContext.endpoints[]` struct read the wrong words on Athena.
    pub fn output_endpoint(&self, ctx64: bool, dci: usize) -> EndpointContext {
        let stride = if ctx64 { 64usize } else { 32 };
        unsafe {
            *((self.output_ctx.virt as *const u8).add(stride * dci) as *const EndpointContext)
        }
    }

    /// Rebuild the input page for an EP0 Max Packet Size update via Evaluate
    /// Context (xHCI §4.6.7: A1-only; the slot context is not evaluated).
    /// The EP0 context is the live OUTPUT context (the xHC's current state)
    /// with the EP State field zeroed (RsvdZ in input contexts) and the new
    /// MPS applied. Returns the input context physical address to submit.
    pub fn prepare_evaluate_ep0_mps(&mut self, new_mps: u16, ctx64: bool) -> u64 {
        let mut ep0 = self.output_endpoint(ctx64, 1);
        ep0.data[0] &= !0x07; // EP State: RsvdZ in input contexts
        ep0.set_max_packet_size(new_mps);

        unsafe {
            core::ptr::write_bytes(self.input_ctx.virt as *mut u8, 0, 4096);
        }
        let ic = self.input_context();
        ic.control.add_endpoint(0); // A1 = EP0 (DCI 1)
        ic.device.endpoints[0] = ep0;
        if ctx64 {
            self.expand_input_context_csz64();
        }
        self.max_packet_size_ep0 = new_mps;
        self.input_ctx.phys
    }

    fn input_control_at(virt: u64) -> *const InputControlContext {
        virt as *const InputControlContext
    }

    fn device_context_at_spec(virt: u64) -> *const DeviceContext {
        // Safety: after pack, device context begins at byte 64 of the input context page.
        unsafe { (virt as *const u8).add(INPUT_CTX_DEVICE_OFFSET_SPEC) as *const DeviceContext }
    }

    fn device_context_at_compact(virt: u64) -> *const DeviceContext {
        unsafe { (virt as *const u8).add(INPUT_CTX_DEVICE_OFFSET_COMPACT) as *const DeviceContext }
    }

    /// Serial dump of Input Context fields consumed by **Configure Endpoint** (spec layout).
    pub fn log_configure_input_context(&self, ep: HidInterruptEndpoint, spec_layout: bool) {
        let virt = self.input_ctx.virt;
        let ic = unsafe { &*Self::input_control_at(virt) };
        let dev = unsafe {
            if spec_layout {
                &*Self::device_context_at_spec(virt)
            } else {
                &*Self::device_context_at_compact(virt)
            }
        };
        let add = ic.add_flags;
        let drop = ic.drop_flags;
        let ce = dev.slot.context_entries();
        let expected_add = 1 | (1 << (ep.ep_index + 1));
        crate::serial_println!(
            "[xhci] ConfigureCtx phys={:#x} layout={} add={:#010x} drop={:#010x} expected_add={:#010x} ce={} cfg={} if={} alt={}",
            self.input_ctx.phys,
            if spec_layout { "spec(+64)" } else { "compact(+32)" },
            add,
            drop,
            expected_add,
            ce,
            ic.configuration_value,
            ic.interface_number,
            ic.alternate_setting,
        );
        for n in 0..=6u8 {
            if add & (1 << n) != 0 {
                let label = match n {
                    0 => "A0 slot",
                    1 => "A1 EP0",
                    2 => "A2",
                    3 => "A3 EP1-OUT",
                    4 => "A4 EP1-IN",
                    5 => "A5",
                    6 => "A6",
                    _ => "?",
                };
                crate::serial_println!("[xhci]   add {}", label);
            }
        }
        if add != expected_add {
            crate::serial_println!(
                "[xhci]   WARN add_flags mismatch (missing {:+#010x})",
                expected_add & !add
            );
        }
        if ce != ep.ep_index + 1 {
            crate::serial_println!(
                "[xhci]   WARN context_entries={} expected {}",
                ce,
                ep.ep_index + 1
            );
        }
        let idx = ep.ep_index as usize;
        if idx < 31 {
            let ep_ctx = &dev.endpoints[idx];
            crate::serial_println!(
                "[xhci]   endpoints[{}] type={:?} mps={} interval={} esit={} avg_trb={} cerr={} dq={:#x} dcs={}",
                idx,
                ep_ctx.ep_type(),
                ep_ctx.max_packet_size(),
                ep_ctx.interval(),
                ep_ctx.max_esit_payload_lo(),
                ep_ctx.average_trb_length(),
                ep_ctx.error_count(),
                ep_ctx.tr_dequeue_pointer(),
                ep_ctx.dequeue_cycle_state()
            );
        }
    }

    fn input_context(&mut self) -> &mut InputContext {
        // Safety: input_ctx is a full page backing InputContext.
        unsafe { &mut *(self.input_ctx.virt as *mut InputContext) }
    }

    fn device_context(&mut self) -> &mut DeviceContext {
        // Safety: output_ctx is a full page backing DeviceContext.
        unsafe { &mut *(self.output_ctx.virt as *mut DeviceContext) }
    }

    pub fn configure_endpoint(
        &mut self,
        ep_index: u8,
        ep_type: EndpointType,
        max_packet_size: u16,
        max_burst: u8,
        interval: u8,
    ) -> Result<(), XhciError> {
        let idx = ep_index as usize;
        if idx < 31 {
            let mut ep_ctx = EndpointContext::default();
            ep_ctx.set_ep_type(ep_type);
            ep_ctx.set_max_packet_size(max_packet_size);
            ep_ctx.set_max_burst_size(max_burst);
            ep_ctx.set_interval(interval);
            ep_ctx.set_error_count(3);
            ep_ctx.set_average_trb_length(1024);

            if self.transfer_rings[idx].is_none() {
                self.transfer_rings[idx] = Some(TransferRing::new(RING_SEGMENT_SIZE)?);
            }
            let ring_phys = self.transfer_rings[idx].as_ref().map(|r| r.phys_addr());
            if let Some(phys) = ring_phys {
                ep_ctx.set_tr_dequeue_pointer(phys, false);
            }

            self.device_context().endpoints[idx] = ep_ctx;
            self.input_context().control.add_endpoint(ep_index);
            Ok(())
        } else {
            Err(XhciError::InvalidEndpoint)
        }
    }

    pub fn deconfigure_endpoint(&mut self, ep_index: u8) {
        let idx = ep_index as usize;
        if idx < 31 {
            self.device_context().endpoints[idx] = EndpointContext::default();
            self.transfer_rings[idx] = None;
            self.input_context().control.drop_endpoint(ep_index);
        }
    }

    pub fn reset_endpoint(&mut self, ep_index: u8) {
        let idx = ep_index as usize;
        if idx < 31 {
            if let Some(ref mut ring) = self.transfer_rings[idx] {
                ring.enqueue_index = 0;
                ring.dequeue_index = 0;
                ring.cycle_state = true;
            }
        }
    }

    pub fn stop_endpoint(&mut self, _ep_index: u8) {
        // Hardware handles the stop; software just tracks state
    }

    pub fn enqueue_transfer(&mut self, ep_index: u8, trb: Trb) -> Result<usize, XhciError> {
        let idx = ep_index as usize;
        if idx >= 31 {
            return Err(XhciError::InvalidEndpoint);
        }
        match self.transfer_rings[idx] {
            Some(ref mut ring) => ring.enqueue(trb),
            None => Err(XhciError::EndpointNotConfigured),
        }
    }

    pub fn setup_address_context(&mut self, route_string: u32, context_size_64: bool) {
        self.setup_address_context_routed(route_string, None, context_size_64);
    }

    /// Build the Address Device input context, optionally as a device behind a
    /// hub. `parent` carries the parent hub's slot id, the downstream port the
    /// device hangs off, and the hub's TT parameters — required so the xHC can
    /// issue split transactions for low/full-speed devices behind a HS hub.
    pub fn setup_address_context_routed(
        &mut self,
        route_string: u32,
        parent: Option<ParentHub>,
        _context_size_64: bool,
    ) {
        let speed = self.speed as u8;
        let port_id = self.port_id;
        let mps0 = self.max_packet_size_ep0;
        // Only low/full-speed devices behind a high-speed hub need the TT fields
        // populated (xHCI §4.5.4); decide before taking the mutable slot borrow.
        let needs_tt = matches!(self.speed, PortSpeed::LowSpeed | PortSpeed::FullSpeed);
        let ep0_ring_phys = self
            .transfer_rings
            .get(0)
            .and_then(|r| r.as_ref())
            .map(|r| r.phys_addr());

        self.input_context().control = InputControlContext::default();
        self.input_context().control.add_slot_context();
        self.input_context().control.add_endpoint(0);

        let slot = &mut self.input_context().device.slot;
        slot.set_route_string(route_string);
        slot.set_speed(speed);
        // Last valid endpoint context index for EP0 (DCI 1) only.
        slot.set_context_entries(1);
        // root_hub_port is always the TOP-LEVEL root port the chain hangs off,
        // regardless of how many hubs sit between; the route string carries the
        // per-tier downstream port numbers.
        slot.set_root_hub_port(port_id);
        if let Some(p) = parent {
            if needs_tt {
                slot.set_parent_hub_slot(p.slot_id);
                slot.set_parent_port(p.port);
                slot.set_tt_think_time(p.tt_think_time);
                slot.set_mtt(p.multi_tt);
            }
        }

        let ep0 = &mut self.input_context().device.endpoints[0];
        ep0.set_ep_type(EndpointType::Control);
        ep0.set_max_packet_size(mps0);
        ep0.set_error_count(3);
        ep0.set_average_trb_length(8);
        if let Some(phys) = ep0_ring_phys {
            ep0.set_tr_dequeue_pointer(phys, false);
        }
    }
}

/// Parameters describing the hub a device is attached to, needed to address a
/// device on a downstream hub port.
#[derive(Clone, Copy, Debug)]
pub struct ParentHub {
    /// xHC slot id of the parent hub.
    pub slot_id: u8,
    /// Downstream port number on the parent hub (1-based).
    pub port: u8,
    /// Hub's TT think time (slot-context encoding, 0..3).
    pub tt_think_time: u8,
    /// Hub exposes multiple transaction translators.
    pub multi_tt: bool,
}

// ─── Streams ────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct StreamContext {
    pub stream_id: u16,
    pub ring: TransferRing,
    pub dequeue_pointer: u64,
    pub stream_type: u8,
}

impl StreamContext {
    pub fn new(stream_id: u16) -> Result<Self, XhciError> {
        let ring = TransferRing::new(RING_SEGMENT_SIZE)?;
        let dequeue_pointer = ring.phys_addr();
        Ok(Self {
            stream_id,
            ring,
            dequeue_pointer,
            stream_type: 1,
        })
    }
}

#[derive(Debug)]
pub struct StreamArray {
    pub contexts: Vec<StreamContext>,
    pub max_streams: u16,
}

impl StreamArray {
    pub fn new(max_streams: u16) -> Result<Self, XhciError> {
        let mut contexts = Vec::with_capacity(max_streams as usize);
        for i in 0..max_streams {
            contexts.push(StreamContext::new(i + 1)?);
        }
        Ok(Self {
            contexts,
            max_streams,
        })
    }

    pub fn get_stream(&self, stream_id: u16) -> Option<&StreamContext> {
        self.contexts.get((stream_id - 1) as usize)
    }

    pub fn get_stream_mut(&mut self, stream_id: u16) -> Option<&mut StreamContext> {
        self.contexts.get_mut((stream_id - 1) as usize)
    }
}

// ─── Interrupt Handling / MSI ───────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct MsiCapability {
    pub address: u64,
    pub data: u32,
    pub enabled: bool,
    pub multi_message_capable: u8,
    pub multi_message_enable: u8,
    pub is_64bit: bool,
    pub per_vector_masking: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct MsixCapability {
    pub table_size: u16,
    pub table_offset: u32,
    pub table_bir: u8,
    pub pba_offset: u32,
    pub pba_bir: u8,
    pub enabled: bool,
    pub function_mask: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct MsixTableEntry {
    pub address_lo: u32,
    pub address_hi: u32,
    pub data: u32,
    pub vector_control: u32,
}

impl MsixTableEntry {
    pub fn masked(&self) -> bool {
        self.vector_control & 1 != 0
    }
    pub fn address(&self) -> u64 {
        (self.address_lo as u64) | ((self.address_hi as u64) << 32)
    }
}

// ─── xHCI Errors ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XhciError {
    NotReady,
    Timeout,
    RingFull,
    NoSlots,
    InvalidSlot,
    InvalidEndpoint,
    EndpointNotConfigured,
    CommandFailed(TrbCompletionCode),
    TransferFailed(TrbCompletionCode),
    HostError,
    ResetFailed,
    PortError,
    NoMemory,
    InvalidState,
}

// ─── xHCI Controller ────────────────────────────────────────────────────────

pub const MAX_DEVICE_SLOTS: usize = 256;
pub const MAX_INTERRUPTERS: usize = 1024;
pub const MAX_PORTS: usize = 256;

pub struct XhciController {
    pub mmio_base: u64,
    pub cap_regs: Option<CapabilityRegisters>,
    pub op_regs_offset: u32,
    pub max_slots: u8,
    pub max_ports: u8,
    pub max_interrupters: u16,
    pub page_size: u32,
    pub context_size_64: bool,
    pub dcbaa: Option<Dcbaa>,
    pub command_ring: Option<CommandRing>,
    pub event_ring: Option<EventRing>,
    pub scratchpad: Option<ScratchpadBuffers>,
    pub device_slots: Vec<Option<DeviceSlot>>,
    pub port_speeds: Vec<PortSpeed>,
    pub extended_caps: Vec<ExtendedCapEntry>,
    pub msi: Option<MsiCapability>,
    pub msix: Option<MsixCapability>,
    pub running: bool,
    pub initialized: bool,
    /// True when this is QEMU's emulated `qemu-xhci` (set from the PCI vendor:
    /// Red Hat 0x1b36). The reliable QEMU discriminator — QEMU-TCG does NOT
    /// set the hypervisor CPUID bit, so `cpuid_hv_vendor_is_qemu()` reports
    /// bare-metal under TCG. Drives `rewind_ep0_if_qemu` (QEMU needs the EP0
    /// rewind between control transfers; real AMD silicon halts on it).
    pub qemu_doorbell_offset: bool,
    /// PCI vendor id of the xHCI controller (set by `init`); 0x1b36 = QEMU.
    pub pci_vendor_id: u16,
    /// Transfer (and other) events received while waiting for a command completion.
    deferred_events: Vec<Trb>,
    /// Diagnostic-only: set once `wait_for_transfer` first observes the
    /// controller wedged (USBSTS HCE/HSE) so the health dump is logged exactly
    /// once instead of for every fast-failed dead probe. Does NOT gate behaviour.
    host_error_logged: bool,
}

#[derive(Debug, Clone)]
pub struct ExtendedCapEntry {
    pub id: u8,
    pub offset: u32,
    pub data: Vec<u32>,
}

impl XhciController {
    pub const fn new() -> Self {
        Self {
            mmio_base: 0,
            cap_regs: None,
            op_regs_offset: 0,
            max_slots: 0,
            max_ports: 0,
            max_interrupters: 0,
            page_size: 4096,
            context_size_64: false,
            dcbaa: None,
            command_ring: None,
            event_ring: None,
            scratchpad: None,
            device_slots: Vec::new(),
            port_speeds: Vec::new(),
            extended_caps: Vec::new(),
            msi: None,
            msix: None,
            running: false,
            initialized: false,
            qemu_doorbell_offset: false,
            pci_vendor_id: 0,
            deferred_events: Vec::new(),
            host_error_logged: false,
        }
    }

    fn defer_event(&mut self, trb: Trb) {
        if self.deferred_events.len() < 32 {
            self.deferred_events.push(trb);
        }
    }

    fn take_deferred_transfer(&mut self) -> Option<Trb> {
        let pos = self
            .deferred_events
            .iter()
            .position(|t| t.trb_type() == TrbType::TransferEvent)?;
        Some(self.deferred_events.remove(pos))
    }

    fn check_transfer_event(trb: Trb) -> Result<Trb, XhciError> {
        let code = TrbCompletionCode::from_u8(((trb.status >> 24) & 0xFF) as u8);
        match code {
            TrbCompletionCode::Success | TrbCompletionCode::ShortPacket => Ok(trb),
            other => Err(XhciError::TransferFailed(other)),
        }
    }

    /// xHCI §4.7 doorbell target = endpoint Device Context Index (DCI).
    ///
    /// Our `ep_index` is the `endpoints[]` array index, which equals `DCI - 1`
    /// (`endpoints[0]` = EP0 = DCI 1; EP1-IN `0x81` = `endpoints[2]` = DCI 3), so
    /// the doorbell target is ALWAYS `ep_index + 1`. This is the standard on QEMU
    /// **and** real hardware — the transfer-event `Endpoint ID` field is this same
    /// DCI, so `doorbell_target(ep) == ep_id` only holds with this mapping.
    ///
    /// (Previously this was gated behind a `qemu_doorbell_offset` flag that fell
    /// back to bare `ep_index` off-QEMU. That left the HID interrupt-IN doorbell
    /// ringing DCI-1 on real hardware: the mouse enumerated via EP0 — lit up —
    /// then was never polled, suspended, and the LED went dark. Always use DCI.)
    pub fn doorbell_target(&self, ep_index: u8) -> u8 {
        ep_index.saturating_add(1)
    }

    // ─── MMIO volatile access helpers ────────────────────────────────────────

    fn read_reg32(&self, offset: u64) -> u32 {
        // Safety: offset is relative to mmio_base which is the BAR0 mapped address.
        // The controller's MMIO region is mapped uncacheable by PCI enumeration.
        unsafe { core::ptr::read_volatile((self.mmio_base + offset) as *const u32) }
    }

    fn write_reg32(&self, offset: u64, value: u32) {
        // Safety: writes to a memory-mapped hardware register in the xHCI BAR0 region.
        unsafe { core::ptr::write_volatile((self.mmio_base + offset) as *mut u32, value) }
    }

    fn read_reg64(&self, offset: u64) -> u64 {
        // Safety: 64-bit MMIO read split into two 32-bit volatile reads (low then high).
        unsafe {
            let lo = core::ptr::read_volatile((self.mmio_base + offset) as *const u32) as u64;
            let hi = core::ptr::read_volatile((self.mmio_base + offset + 4) as *const u32) as u64;
            lo | (hi << 32)
        }
    }

    fn write_reg64(&self, offset: u64, value: u64) {
        // Safety: 64-bit MMIO write split into two 32-bit volatile writes (low first).
        unsafe {
            core::ptr::write_volatile((self.mmio_base + offset) as *mut u32, value as u32);
            core::ptr::write_volatile(
                (self.mmio_base + offset + 4) as *mut u32,
                (value >> 32) as u32,
            );
        }
    }

    fn read_reg8(&self, offset: u64) -> u8 {
        // Safety: byte-level volatile read from xHCI MMIO space.
        unsafe { core::ptr::read_volatile((self.mmio_base + offset) as *const u8) }
    }

    fn read_reg16(&self, offset: u64) -> u16 {
        // Safety: 16-bit volatile read from xHCI MMIO space.
        unsafe { core::ptr::read_volatile((self.mmio_base + offset) as *const u16) }
    }

    /// Operational register base = mmio_base + caplength
    fn op_base(&self) -> u64 {
        self.op_regs_offset as u64
    }

    fn read_op_reg(&self, offset: u64) -> u32 {
        self.read_reg32(self.op_base() + offset)
    }

    /// One-line controller-health dump for timeout paths. Distinguishes
    /// "the device didn't answer" (controller fine, endpoint problem) from
    /// "the controller died" — HCH (halted) / HSE (host system error, i.e.
    /// a DMA the platform rejected) / HCE (controller error) — which is the
    /// difference between debugging a USB device and debugging our DMA
    /// addresses. Photographable on the bare-metal panel ([xhci] prefix).
    fn log_controller_health(&self, ctx: &str) {
        let sts = self.read_op_reg(0x04);
        crate::serial_println!(
            "[xhci] {}: USBSTS={:#010x} HCH={} HSE={} HCE={}",
            ctx,
            sts,
            sts & USBSTS_HCH != 0,
            sts & USBSTS_HSE != 0,
            sts & USBSTS_HCE != 0,
        );
    }

    fn write_op_reg(&self, offset: u64, value: u32) {
        self.write_reg32(self.op_base() + offset, value)
    }

    fn read_op_reg64(&self, offset: u64) -> u64 {
        self.read_reg64(self.op_base() + offset)
    }

    fn write_op_reg64(&self, offset: u64, value: u64) {
        self.write_reg64(self.op_base() + offset, value)
    }

    fn doorbell_base(&self) -> u64 {
        if let Some(ref cap) = self.cap_regs {
            cap.doorbell_array_offset() as u64
        } else {
            0
        }
    }

    fn runtime_base(&self) -> u64 {
        if let Some(ref cap) = self.cap_regs {
            cap.runtime_register_space_offset() as u64
        } else {
            0
        }
    }

    fn port_reg_base(&self, port: u8) -> u64 {
        self.op_base() + 0x400 + ((port as u64 - 1) * 0x10)
    }

    // ─── Initialization ──────────────────────────────────────────────────────

    pub fn initialize(&mut self, mmio_base: u64) -> Result<(), XhciError> {
        self.mmio_base = mmio_base;

        let cap_regs = self.read_capability_registers()?;
        self.max_slots = cap_regs.max_slots();
        self.max_ports = cap_regs.max_ports();
        self.max_interrupters = cap_regs.max_intrs();
        self.op_regs_offset = cap_regs.caplength as u32;
        self.context_size_64 = cap_regs.csz();
        self.cap_regs = Some(cap_regs);

        self.wait_controller_ready()?;
        self.reset_controller()?;
        self.wait_controller_ready()?;

        self.page_size = self.read_page_size();

        self.dcbaa = Some(Dcbaa::new(self.max_slots)?);
        self.command_ring = Some(CommandRing::new()?);
        self.event_ring = Some(EventRing::new(RING_SEGMENT_SIZE)?);

        let scratchpad_count = cap_regs.max_scratchpad_bufs();
        if scratchpad_count > 0 {
            self.scratchpad = Some(ScratchpadBuffers::new(
                scratchpad_count,
                self.page_size as usize,
            )?);
            if let (Some(ref mut dcbaa), Some(ref scratchpad)) = (&mut self.dcbaa, &self.scratchpad)
            {
                dcbaa.set_device_context(0, scratchpad.array_phys_addr());
            }
        }

        self.device_slots = Vec::with_capacity(self.max_slots as usize);
        for _ in 0..self.max_slots {
            self.device_slots.push(None);
        }

        self.port_speeds = alloc::vec![PortSpeed::Undefined; self.max_ports as usize];

        self.parse_extended_capabilities();
        self.claim_ownership();
        self.configure_max_device_slots();
        self.set_dcbaa_pointer();
        self.set_command_ring_pointer();
        self.setup_interrupter();

        self.start()?;
        // Informational only: doorbell targets are ALWAYS the endpoint DCI
        // (ep_index + 1) on both QEMU and real hardware — see doorbell_target().
        // QEMU detection: the qemu-xhci controller's PCI vendor is Red Hat
        // (0x1b36). This is reliable under TCG, where the hypervisor CPUID
        // bit is unset and `cpuid_hv_vendor_is_qemu()` wrongly reports
        // bare-metal (kept as a secondary signal for KVM/accelerated runs).
        self.qemu_doorbell_offset = self.pci_vendor_id == 0x1b36 || cpuid_hv_vendor_is_qemu();
        crate::serial_println!(
            "[xhci] doorbell targets: DCI (ep_index+1); host={}",
            if self.qemu_doorbell_offset {
                "QEMU"
            } else {
                "bare-metal"
            }
        );
        if !self.context_size_64 {
            crate::serial_println!(
                "[xhci] 32-byte device contexts (CSZ=0); using compact AML-style input layout"
            );
        }
        self.initialized = true;
        Ok(())
    }

    fn read_capability_registers(&self) -> Result<CapabilityRegisters, XhciError> {
        Ok(CapabilityRegisters {
            caplength: self.read_reg8(0x00),
            reserved: 0,
            hciversion: self.read_reg16(0x02),
            hcsparams1: self.read_reg32(0x04),
            hcsparams2: self.read_reg32(0x08),
            hcsparams3: self.read_reg32(0x0C),
            hccparams1: self.read_reg32(0x10),
            dboff: self.read_reg32(0x14),
            rtsoff: self.read_reg32(0x18),
            hccparams2: self.read_reg32(0x1C),
        })
    }

    fn wait_controller_ready(&self) -> Result<(), XhciError> {
        for _ in 0..100_000 {
            let usbsts = self.read_op_reg(0x04);
            if usbsts & USBSTS_CNR == 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(XhciError::Timeout)
    }

    fn reset_controller(&self) -> Result<(), XhciError> {
        let usbcmd = self.read_op_reg(0x00);
        self.write_op_reg(0x00, usbcmd | USBCMD_HCRST);

        for _ in 0..100_000 {
            let cmd = self.read_op_reg(0x00);
            if cmd & USBCMD_HCRST == 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(XhciError::ResetFailed)
    }

    fn read_page_size(&self) -> u32 {
        let raw = self.read_op_reg(0x08);
        if raw == 0 {
            return 4096;
        }
        1u32 << (raw.trailing_zeros() + 12)
    }

    fn parse_extended_capabilities(&mut self) {
        if let Some(ref cap_regs) = self.cap_regs {
            let xecp = cap_regs.xecp();
            if xecp == 0 {
                return;
            }

            let mut offset = (xecp as u64) * 4;
            for _ in 0..64 {
                let dword0 = self.read_reg32(offset);
                let id = (dword0 & 0xFF) as u8;
                let next = ((dword0 >> 8) & 0xFF) as u8;

                self.extended_caps.push(ExtendedCapEntry {
                    id,
                    offset: offset as u32,
                    data: alloc::vec![dword0],
                });

                if next == 0 {
                    break;
                }
                offset += (next as u64) * 4;
            }
        }
    }

    fn claim_ownership(&self) {
        for cap in &self.extended_caps {
            if cap.id == ExtendedCapabilityId::UsbLegacySupport as u8 {
                let offset = cap.offset as u64;
                // Set OS Owned Semaphore (bit 24)
                let val = self.read_reg32(offset);
                self.write_reg32(offset, val | (1 << 24));
                // Wait for BIOS to release (bit 16 clears)
                for _ in 0..100_000 {
                    let v = self.read_reg32(offset);
                    if v & (1 << 16) == 0 {
                        break;
                    }
                    core::hint::spin_loop();
                }
                break;
            }
        }
    }

    fn configure_max_device_slots(&self) {
        self.write_op_reg(0x38, self.max_slots as u32);
    }

    fn set_dcbaa_pointer(&self) {
        if let Some(ref dcbaa) = self.dcbaa {
            self.write_op_reg64(0x30, dcbaa.phys_addr());
        }
    }

    fn set_command_ring_pointer(&self) {
        if let Some(ref cmd_ring) = self.command_ring {
            self.write_op_reg64(0x18, cmd_ring.phys_addr() | 1);
        }
    }

    fn setup_interrupter(&self) {
        if let Some(ref event_ring) = self.event_ring {
            let rt_base = self.runtime_base();
            let intr0 = rt_base + 0x20;

            // ERSTSZ
            self.write_reg32(intr0 + 0x08, 1);
            // ERDP
            self.write_reg64(intr0 + 0x18, event_ring.dequeue_pointer());
            // ERSTBA
            self.write_reg64(intr0 + 0x10, event_ring.erst_phys_addr());
            // Enable interrupter
            let iman = self.read_reg32(intr0);
            self.write_reg32(intr0, iman | IMAN_IE);
        }
    }

    fn start(&mut self) -> Result<(), XhciError> {
        let usbcmd = self.read_op_reg(0x00);
        self.write_op_reg(0x00, usbcmd | USBCMD_RS | USBCMD_INTE);

        for _ in 0..100_000 {
            let sts = self.read_op_reg(0x04);
            if sts & USBSTS_HCH == 0 {
                self.running = true;
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(XhciError::Timeout)
    }

    pub fn stop(&mut self) -> Result<(), XhciError> {
        let usbcmd = self.read_op_reg(0x00);
        self.write_op_reg(0x00, usbcmd & !USBCMD_RS);

        for _ in 0..100_000 {
            let sts = self.read_op_reg(0x04);
            if sts & USBSTS_HCH != 0 {
                self.running = false;
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(XhciError::Timeout)
    }

    /// Spin until a command-completion event arrives (polled; no MSI yet).
    pub fn wait_for_command(&mut self, _spins: u32) -> Result<Trb, XhciError> {
        // 100ms wall-clock, same rationale as wait_for_transfer below: the
        // old `spins` iteration count expired in ~100µs on Athena's 4+ GHz
        // cores, far short of a real Address Device command (the xHC runs
        // an actual SET_ADDRESS bus transaction, milliseconds at low speed)
        // while always passing on instant-completion QEMU.
        let mut result: Option<Trb> = None;
        let got = crate::hpet::spin_until_us(100_000, || {
            let events = self.poll_events();
            for trb in events {
                match trb.trb_type() {
                    TrbType::CommandCompletionEvent if result.is_none() => {
                        result = Some(trb);
                    }
                    _ => self.defer_event(trb),
                }
            }
            result.is_some()
        });
        if got {
            if let Some(trb) = result {
                return Ok(trb);
            }
        }
        self.log_controller_health("command timeout");
        Err(XhciError::Timeout)
    }

    /// Poll the event ring until a transfer-completion event arrives.
    pub fn wait_for_transfer(&mut self, _spins: u32) -> Result<Trb, XhciError> {
        if let Some(trb) = self.take_deferred_transfer() {
            return Self::check_transfer_event(trb);
        }
        // 100ms wall-clock — generous for any USB control transfer
        // (spec allows 5s, practical devices respond in microseconds).
        // The `_spins` parameter is now ignored; previously it was a
        // CPU-iteration count that meant nothing across CPU speeds —
        // 500_000 spins on QEMU TCG ≈ 500ms, on a 4.5 GHz core ≈ 110µs.
        // The single biggest boot-time line item in safe-mode QEMU was
        // usb_msc::probe_all_msc → get_descriptor → here at 500ms per
        // unenumerated slot. Now bounded at 100ms wall-clock.
        //
        // HCE GUARD: once USBSTS latches a fatal error (HCE host-controller-error
        // / HSE host-system-error), the controller produces NO further events —
        // any transfer that hasn't already completed never will. We still POLL
        // (a completion already in flight, e.g. a downstream-hub HID config whose
        // event is microseconds away, must still succeed), but cap the wait at a
        // short grace instead of the full 100ms. Pre-guard, QEMU's emulated xHCI
        // wedged enumerating the empty downstream ports of its USB3 hub and then
        // ground 100ms × ~168 dead probes — ~9s, the dominant Tier-6 boot cost.
        // On real hardware the controller doesn't wedge (HCE stays clear), so
        // `wedged` is false and the full 100ms is preserved — iron behaviour is
        // unchanged.
        let wedged = self.read_op_reg(0x04) & (USBSTS_HCE | USBSTS_HSE) != 0;
        let timeout_us = if wedged { 5_000 } else { 100_000 };
        let mut completion: Option<Trb> = None;
        let mut deferred: Vec<Trb> = Vec::new();
        let got = crate::hpet::spin_until_us(timeout_us, || {
            let events = self.poll_events();
            for trb in events {
                match trb.trb_type() {
                    TrbType::TransferEvent => {
                        completion = Some(trb);
                        return true;
                    }
                    TrbType::CommandCompletionEvent => deferred.push(trb),
                    _ => {}
                }
            }
            false
        });
        for trb in deferred {
            self.defer_event(trb);
        }
        if got {
            if let Some(trb) = completion {
                return Self::check_transfer_event(trb);
            }
        }
        if wedged {
            // Controller wedged — fail fast. Log the health dump exactly once
            // (the first observation) so the wedge is visible on iron without
            // the cascade of dead probes re-spamming it; then fail quietly.
            if !self.host_error_logged {
                self.host_error_logged = true;
                self.log_controller_health("controller wedged (HCE/HSE) — failing transfers fast");
            }
            return Err(XhciError::Timeout);
        }
        crate::serial_println!(
            "[xhci] wait_for_transfer: timeout (deferred={})",
            self.deferred_events.len()
        );
        if let Some(ref event_ring) = self.event_ring {
            crate::serial_println!(
                "[xhci]   event ring dequeue_idx={}",
                event_ring.dequeue_index
            );
        }
        self.log_controller_health("transfer timeout");
        Err(XhciError::Timeout)
    }

    /// Standard USB GET_DESCRIPTOR into a DMA buffer (control-IN).
    pub fn get_descriptor(
        &mut self,
        slot_id: u8,
        desc_type: u8,
        desc_index: u8,
        length: u16,
    ) -> Result<Vec<u8>, XhciError> {
        let len = length as usize;
        if len == 0 || len > 4096 {
            return Err(XhciError::InvalidState);
        }
        let page = alloc_dma_page()?;
        let buf = unsafe { core::slice::from_raw_parts_mut(page.virt as *mut u8, len) };
        // USB setup wValue: (descriptor type << 8) | index; encoded little-endian in bytes 2..3.
        let setup: [u8; 8] = [
            0x80,
            0x06,
            desc_index,
            desc_type,
            0x00,
            0x00,
            (length & 0xFF) as u8,
            (length >> 8) as u8,
        ];
        self.submit_control_transfer(slot_id, &setup, Some((buf, page.phys)), true)?;
        let event = self.wait_for_transfer(500_000)?;
        // Transfer event status[23:0] = residual (bytes not transferred on IN).
        let residual = (event.status & 0x00FF_FFFF) as usize;
        let got = len.saturating_sub(residual);
        let out = buf[..got].to_vec();
        // Rewind EP0 for the next control transfer (QEMU only — see
        // rewind_ep0_if_qemu; bare metal uses natural ring advancement).
        self.rewind_ep0_if_qemu(slot_id);
        // Transfer is complete (wait_for_transfer returned) and the data is
        // copied into `out`, so the DMA page is no longer referenced by the xHC
        // — return its frame instead of leaking it (every descriptor read leaked
        // a 4 KiB page before this). Error paths above (`?`) deliberately leak:
        // a timed-out transfer may still have a TD pointing at the buffer.
        page.free();
        Ok(out)
    }

    /// Fetch a HID REPORT descriptor (type 0x22) from an interface. Unlike the
    /// standard device-targeted GET_DESCRIPTOR, this is an INTERFACE request
    /// (bmRequestType 0x81, wIndex = interface number) per USB HID 1.11 §7.1.1.
    /// Returns the raw report-descriptor bytes for `raehid` to parse.
    pub fn get_hid_report_descriptor(
        &mut self,
        slot_id: u8,
        interface: u16,
        length: u16,
    ) -> Result<Vec<u8>, XhciError> {
        let len = length as usize;
        if len == 0 || len > 4096 {
            return Err(XhciError::InvalidState);
        }
        let page = alloc_dma_page()?;
        let buf = unsafe { core::slice::from_raw_parts_mut(page.virt as *mut u8, len) };
        // bmRequestType=0x81 (device→host, standard, INTERFACE), bRequest=0x06
        // (GET_DESCRIPTOR), wValue=(0x22<<8) (Report descriptor, index 0),
        // wIndex=interface, wLength=len.
        let setup: [u8; 8] = [
            0x81,
            0x06,
            0x00,
            0x22,
            (interface & 0xFF) as u8,
            (interface >> 8) as u8,
            (length & 0xFF) as u8,
            (length >> 8) as u8,
        ];
        self.submit_control_transfer(slot_id, &setup, Some((buf, page.phys)), true)?;
        let event = self.wait_for_transfer(500_000)?;
        let residual = (event.status & 0x00FF_FFFF) as usize;
        let got = len.saturating_sub(residual);
        let out = buf[..got].to_vec();
        self.rewind_ep0_if_qemu(slot_id);
        // DMA complete + copied out — free the transient page (see get_descriptor).
        page.free();
        Ok(out)
    }

    /// Standard USB GET_DESCRIPTOR (device), 18-byte descriptor in a DMA page.
    pub fn get_device_descriptor(&mut self, slot_id: u8) -> Result<[u8; 18], XhciError> {
        let data = self.get_descriptor(slot_id, 1, 0, 18)?;
        let mut out = [0u8; 18];
        out.copy_from_slice(&data[..18.min(data.len())]);
        Ok(out)
    }

    /// GET_DESCRIPTOR with one GENTLE stall-retry — no destructive `recover_ep0`.
    /// A control STALL is a protocol stall the device auto-clears on the next
    /// SETUP (USB 2.0 §8.5.3.4), so a single re-issue after a short settle is the
    /// correct recovery for the LowSpeed multi-packet quirk that stalls a
    /// full-length control-IN (the Athena port-2 keyboard). Used for the config-
    /// descriptor reads in HID/MSC bring-up so a transient stall there recovers
    /// instead of `?`-abandoning the device. (The heavy Stop/Reset-Endpoint path
    /// HCE-halts AMD silicon — CLAUDE §10 — so it is deliberately NOT used here.)
    fn get_descriptor_retry(
        &mut self,
        slot_id: u8,
        desc_type: u8,
        desc_index: u8,
        length: u16,
    ) -> Result<Vec<u8>, XhciError> {
        match self.get_descriptor(slot_id, desc_type, desc_index, length) {
            Ok(d) => Ok(d),
            Err(e) => {
                crate::hpet::spin_until_us(5_000, || false);
                self.get_descriptor(slot_id, desc_type, desc_index, length)
                    .map_err(|_| e)
            }
        }
    }

    /// SET_CONFIGURATION (wValue = configuration number).
    pub fn set_configuration(&mut self, slot_id: u8, config_value: u8) -> Result<(), XhciError> {
        let setup: [u8; 8] = [0x00, 0x09, config_value, 0x00, 0x00, 0x00, 0x00, 0x00];
        self.submit_control_transfer(slot_id, &setup, None, false)?;
        let res = self.wait_for_transfer(500_000);
        // Rewind EP0 like every other EP0 helper (get_descriptor,
        // control_in_vec, control_out_nodata) — QEMU latches HCE on the NEXT
        // control transfer if the ring isn't rewound. This used to be the
        // caller's job, which the non-boot-HID path (usb-tablet, protocol 0)
        // missed: its bring-up ended here, and usb_msc::probe_all_msc's later
        // GET_DESCRIPTOR to that slot timed out with HCE latched. Rewind even
        // on error so a failed SET_CONFIGURATION doesn't poison the next
        // transfer. No-op on bare metal (vendor-gated).
        self.rewind_ep0_if_qemu(slot_id);
        res.map(|_| ())
    }

    /// USB SET_INTERFACE (standard request 0x0B): select alternate setting `alt`
    /// of `interface`. UAC streaming endpoints live on alt 1 (alt 0 is the
    /// zero-bandwidth default), so activating the alt is what arms the
    /// isochronous endpoint for data. EP0 is rewound (QEMU) like the other
    /// control helpers so the next transfer doesn't latch HCE.
    pub fn set_interface(&mut self, slot_id: u8, interface: u8, alt: u8) -> Result<(), XhciError> {
        let setup: [u8; 8] = [0x01, 0x0B, alt, 0x00, interface, 0x00, 0x00, 0x00];
        self.submit_control_transfer(slot_id, &setup, None, false)?;
        let res = self.wait_for_transfer(500_000);
        self.rewind_ep0_if_qemu(slot_id);
        res.map(|_| ())
    }

    /// Configure interrupt-IN endpoint contexts and issue Configure Endpoint command.
    pub fn configure_hid_interrupt(
        &mut self,
        slot_id: u8,
        config_value: u8,
        ep: HidInterruptEndpoint,
    ) -> Result<(), XhciError> {
        let idx = (slot_id - 1) as usize;
        let ctx64 = self.context_size_64;
        let input_phys = {
            let slot = self.device_slots[idx]
                .as_mut()
                .ok_or(XhciError::InvalidSlot)?;
            slot.prepare_configure_hid(config_value, ep)?;
            let out = unsafe { &*(slot.output_ctx.virt as *const DeviceContext) };
            let out_ep0 = slot.output_endpoint(ctx64, 1);
            crate::serial_println!(
                "[xhci]   out before cfg: slot={:?} ce={} ep0[0]={:?} dq={:#x}",
                out.slot.slot_state(),
                out.slot.context_entries(),
                out_ep0.ep_state(),
                out_ep0.tr_dequeue_pointer()
            );
            // The input context stays in the COMPACT layout (ICC@0, device@+32);
            // configure_endpoint_with_ptr below expands it to 64-byte stride for
            // CSZ=1 controllers. The old `use_spec_layout`/pack_input_context_for_
            // configure path (move device to +64) was dead (hardcoded false) AND
            // mutually exclusive with that expand — pack moves the slot to +64,
            // then expand reads the now-zeroed +32 as the "slot" and overwrites the
            // real one with zeros, crashing the controller. Removed to kill the
            // dormant footgun; compact + expand is the proven QEMU+AMD path.
            slot.log_configure_input_context(ep, false);
            slot.input_ctx.phys
        };
        self.configure_endpoint_with_ptr(slot_id, input_phys)
    }

    pub fn submit_interrupt_in(
        &mut self,
        slot_id: u8,
        ep_index: u8,
        buf_phys: u64,
        length: u32,
    ) -> Result<(), XhciError> {
        let idx = (slot_id - 1) as usize;
        let slot = self.device_slots[idx]
            .as_mut()
            .ok_or(XhciError::InvalidSlot)?;

        let mut trb = Trb::new();
        trb.set_trb_type(TrbType::Normal);
        trb.parameter = buf_phys;
        trb.status = length;
        trb.control |= 1 << 16;
        trb.set_ioc(true);
        slot.enqueue_transfer(ep_index, trb)?;

        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        let db = self.doorbell_target(ep_index);
        // ONE-SHOT only. This runs on EVERY interrupt-IN re-arm — i.e. once per
        // drained HID report — so since the EDF drain promotion it fires hundreds
        // of times/s. On iron each serial_println blocks CPU0 ~5 ms on the 115200
        // UART + blits a glyph row over the desktop (iron T1745: the user couldn't
        // use the machine for the doorbell/report flood, and that CPU0 tax is
        // itself input/compositor latency). The doorbell *target* computation was
        // historically wrong off-QEMU (see the ep_index/target note above), so we
        // keep proof of the FIRST one and then go silent.
        static FIRST_INT_DOORBELL: core::sync::atomic::AtomicBool =
            core::sync::atomic::AtomicBool::new(true);
        if FIRST_INT_DOORBELL.swap(false, core::sync::atomic::Ordering::Relaxed) {
            crate::serial_println!(
                "[xhci] interrupt-IN doorbell ep_index={} target={} (first; per-report log suppressed)",
                ep_index,
                db
            );
        }
        self.ring_doorbell_device(slot_id, db, 0);
        Ok(())
    }

    /// One interrupt-IN report read after HID endpoint is configured.
    pub fn poll_hid_keyboard_report(&mut self, slot_id: u8) -> Result<(), XhciError> {
        let idx = (slot_id - 1) as usize;
        let (ep_index, report_len, input_id, buf_virt, buf_phys, protocol) = {
            let slot = self.device_slots[idx]
                .as_ref()
                .ok_or(XhciError::InvalidSlot)?;
            let iface = slot
                .hid_interfaces
                .first()
                .ok_or(XhciError::EndpointNotConfigured)?;
            let ep = iface.ep;
            let len = ep.max_packet_size.max(8) as u32;
            (
                ep.ep_index,
                len,
                iface.input_device_id,
                iface.report_buf.virt,
                iface.report_buf.phys,
                ep.protocol,
            )
        };

        self.submit_interrupt_in(slot_id, ep_index, buf_phys, report_len)?;
        self.wait_for_transfer(50_000)?;

        let report =
            unsafe { core::slice::from_raw_parts(buf_virt as *const u8, report_len as usize) };
        crate::serial_println!("[xhci] HID report: {:02x?}", &report[..report.len().min(8)]);
        if input_id != 0 {
            if protocol == crate::xhci_desc::HID_PROTO_MOUSE {
                crate::usb_hid::dispatch_boot_mouse(input_id, report);
            } else {
                crate::usb_hid::dispatch_boot_report(input_id, report);
            }
        }
        Ok(())
    }

    /// Arm one pending interrupt-IN transfer for every slot with a configured
    /// HID endpoint. The controller leaves the TRB pending (NAK) until the
    /// device produces a report, so call this once after bring-up; re-arming
    /// happens in [`Self::service_hid_reports`] on each completion.
    pub fn arm_hid_interrupts(&mut self) -> usize {
        let mut armed = 0;
        for slot_id in 1..=self.device_slots.len() as u8 {
            // Snapshot each interface's (ep_index, len, phys) under an immutable
            // borrow, then arm — a composite device contributes several.
            let ifaces: Vec<(u8, u32, u64)> =
                match self.device_slots[(slot_id - 1) as usize].as_ref() {
                    Some(slot) => slot
                        .hid_interfaces
                        .iter()
                        .map(|i| {
                            (
                                i.ep.ep_index,
                                i.ep.max_packet_size.max(8) as u32,
                                i.report_buf.phys,
                            )
                        })
                        .collect(),
                    None => Vec::new(),
                };
            for (ep_index, len, phys) in ifaces {
                // Multi-buffer: queue HID_TD_RING interrupt-IN TDs into distinct
                // report slots so the xHC can buffer a batch of reports between
                // drains (see HID_TD_RING). Count the endpoint armed if at least
                // the first TD posted; the rest deepen the buffer.
                let mut any = false;
                let mut queued = 0u64;
                for k in 0..HID_TD_RING {
                    let slot_phys = phys + k * HID_SLOT_SIZE;
                    if self
                        .submit_interrupt_in(slot_id, ep_index, slot_phys, len)
                        .is_ok()
                    {
                        any = true;
                        queued += 1;
                    }
                }
                // Record how many TDs are actually outstanding so the drain loop
                // can top the endpoint back up if it ever drains to empty.
                if let Some(slot) = self.device_slots[(slot_id - 1) as usize].as_mut() {
                    if let Some(iface) = slot
                        .hid_interfaces
                        .iter_mut()
                        .find(|i| i.ep.ep_index == ep_index)
                    {
                        iface.outstanding_tds = queued;
                    }
                }
                if any {
                    armed += 1;
                }
            }
        }
        armed
    }

    /// Poll the event ring once; dispatch completed HID interrupt-IN reports to
    /// the input subsystem and re-arm the endpoint. Returns reports dispatched.
    ///
    /// QEMU posts no event on NAK (the TRB stays pending), so events seen here
    /// are real completions or errors — re-arming only on those keeps exactly
    /// one transfer outstanding per endpoint.
    pub fn service_hid_reports(&mut self) -> usize {
        let events = self.poll_events();
        let mut dispatched = 0;
        for trb in events {
            if trb.trb_type() != TrbType::TransferEvent {
                continue;
            }
            let slot_id = ((trb.control >> 24) & 0xFF) as u8;
            let ep_id = ((trb.control >> 16) & 0x1F) as u8;
            let code = TrbCompletionCode::from_u8(((trb.status >> 24) & 0xFF) as u8);
            if slot_id == 0 || slot_id as usize > self.device_slots.len() {
                continue;
            }
            // Match the completion's endpoint to the right interface on this
            // slot (a composite device has several armed interfaces).
            let info = self.device_slots[(slot_id - 1) as usize]
                .as_ref()
                .and_then(|slot| {
                    slot.hid_interfaces
                        .iter()
                        .find(|i| self.doorbell_target(i.ep.ep_index) == ep_id)
                        .map(|i| {
                            (
                                i.ep.ep_index,
                                i.ep.max_packet_size.max(8) as u32,
                                i.report_buf.virt,
                                i.report_buf.phys,
                                i.input_device_id,
                                i.ep.protocol,
                                i.drain_seq,
                            )
                        })
                });
            let Some((ep_index, len, virt, phys, input_id, protocol, drain_seq)) = info else {
                continue;
            };
            // Multi-buffer round-robin: the xHC completes TDs in order, so this
            // completion owns slot `drain_seq % HID_TD_RING`. Read from + re-arm
            // into THAT slot, then advance drain_seq (below) to keep HID_TD_RING
            // TDs outstanding.
            let phys_base = phys;
            let slot_off = (drain_seq % HID_TD_RING) * HID_SLOT_SIZE;
            let virt = virt + slot_off;
            let phys = phys_base + slot_off;
            // Set by the stall path: re-init the whole ring + reset drain_seq to 0
            // (rather than the usual +1), since a Reset Endpoint cleared every
            // outstanding TD and the slot↔order correspondence with it.
            let mut full_reinit = false;
            // Number of fresh TDs re-armed in this event's branch — folded into
            // the endpoint's outstanding count below (this completion consumed
            // exactly one TD, so net outstanding = +rearmed - 1).
            let mut rearmed: u64 = 0;
            match code {
                TrbCompletionCode::Success | TrbCompletionCode::ShortPacket => {
                    let report =
                        unsafe { core::slice::from_raw_parts(virt as *const u8, len as usize) };
                    // FIRST 16 reports per endpoint ONLY — proves a device
                    // delivers (esp. a newly-debugged keyboard) and shows its byte
                    // layout, a bounded ~16-line one-time cost. NO steady-state
                    // heartbeat here: each line blocks CPU0 ~5 ms on the 115200
                    // UART + blits over the desktop (iron T1745 flood), and the
                    // `[hid-diag] thread alive: iters=N reports=M` line (every ~2 s)
                    // already reports the live count. The old `|| seq % 128 == 0`
                    // re-introduced exactly the per-report flood the EDF drain made
                    // continuous.
                    if drain_seq < 16 {
                        crate::serial_println!(
                            "[xhci] HID report (slot {} seq {}): {:02x?}",
                            slot_id,
                            drain_seq,
                            &report[..report.len().min(8)]
                        );
                    }
                    if input_id != 0 {
                        // A report-protocol device's decode, owned so the slot
                        // borrow drops before dispatch. MICE carry the FULL i32
                        // delta — NOT the i8 boot report — so a high-DPI gaming
                        // mouse (the Athena Razer, 16-bit X/Y) isn't clamped to
                        // ±127/report → cursor crawl (observed on iron). Keyboards
                        // keep the boot-layout bridge (keycodes don't clamp).
                        // Boot-protocol devices (hid_device == None) take the
                        // unchanged path — iron-proven flow preserved.
                        enum Decoded {
                            Mouse(raehid::MouseDelta),
                            Keyboard([u8; 8]),
                        }
                        let decoded: Option<Option<Decoded>> = self.device_slots
                            [(slot_id - 1) as usize]
                            .as_ref()
                            .and_then(|s| {
                                s.hid_interfaces.iter().find(|i| i.ep.ep_index == ep_index)
                            })
                            .and_then(|i| i.hid_device.as_ref())
                            .map(|dev| match dev.kind() {
                                raehid::HidKind::Mouse => {
                                    dev.extract_mouse(report).map(Decoded::Mouse)
                                }
                                _ => dev
                                    .extract_keyboard(report)
                                    .map(|k| Decoded::Keyboard(k.to_boot_report())),
                            });
                        match decoded {
                            // Report-protocol mouse: full-resolution dispatch.
                            Some(Some(Decoded::Mouse(m))) => crate::usb_hid::dispatch_mouse_delta(
                                input_id,
                                m.dx,
                                m.dy,
                                m.wheel,
                                (m.buttons & 0xFF) as u8,
                            ),
                            // Report-protocol keyboard: boot-layout bridge is fine.
                            Some(Some(Decoded::Keyboard(b))) => {
                                crate::usb_hid::dispatch_boot_report(input_id, &b)
                            }
                            // Report device whose decode yielded nothing this frame.
                            Some(None) => {}
                            // Boot-protocol device: route by boot interface protocol.
                            None => {
                                if protocol == crate::xhci_desc::HID_PROTO_MOUSE {
                                    crate::usb_hid::dispatch_boot_mouse(input_id, report);
                                } else {
                                    crate::usb_hid::dispatch_boot_report(input_id, report);
                                }
                            }
                        }
                    }
                    dispatched += 1;
                    if self
                        .submit_interrupt_in(slot_id, ep_index, phys, len)
                        .is_ok()
                    {
                        rearmed += 1;
                    }
                }
                TrbCompletionCode::StallError => {
                    // Reset Endpoint clears the whole transfer ring, so re-arm the
                    // FULL TD ring from slot 0 and reset drain_seq (below) — not a
                    // +1, which would leave the read 1 slot ahead of the writes.
                    let _ = self.reset_endpoint_by_index(slot_id, ep_index);
                    self.sync_ep_enqueue_from_hardware(slot_id, ep_index);
                    for k in 0..HID_TD_RING {
                        if self
                            .submit_interrupt_in(
                                slot_id,
                                ep_index,
                                phys_base + k * HID_SLOT_SIZE,
                                len,
                            )
                            .is_ok()
                        {
                            rearmed += 1;
                        }
                    }
                    full_reinit = true;
                }
                _ => {
                    if self
                        .submit_interrupt_in(slot_id, ep_index, phys, len)
                        .is_ok()
                    {
                        rearmed += 1;
                    }
                }
            }
            // Advance the round-robin index so the next completion for this
            // endpoint drains the next slot. The TD re-armed above lands
            // HID_TD_RING positions later in the FIFO ring, i.e. back at this
            // same modular slot, keeping the slot↔order correspondence. After a
            // stall (full re-init) the ring restarts at slot 0.
            if let Some(slot) = self.device_slots[(slot_id - 1) as usize].as_mut() {
                if let Some(iface) = slot
                    .hid_interfaces
                    .iter_mut()
                    .find(|i| i.ep.ep_index == ep_index)
                {
                    iface.drain_seq = if full_reinit {
                        0
                    } else {
                        iface.drain_seq.wrapping_add(1)
                    };
                    // Outstanding accounting: a stall (full re-init) resets the
                    // ring, so outstanding == the TDs just re-armed; otherwise
                    // this completion consumed one TD and `rearmed` (0 or 1) were
                    // posted back.
                    iface.outstanding_tds = if full_reinit {
                        rearmed
                    } else {
                        iface.outstanding_tds.saturating_sub(1) + rearmed
                    };
                }
            }
        }
        // INDEFINITE-FLOW GUARD (BUG 1): top every armed HID endpoint back up to
        // HID_TD_RING outstanding TDs. The per-completion re-arm above keeps the
        // depth steady in the happy path, but if the ring ever drains to empty
        // (a report burst the drain thread didn't catch in one cycle, a dropped
        // re-arm, or a controller that STOPS the endpoint on an empty ring) then
        // `outstanding_tds` falls toward 0 and — with no TD queued — the
        // controller can never post another completion event to re-arm from. That
        // is the SILENT wedge (no xHCI error logged) the beta-tester reproduced
        // after ~16 events. Re-posting the shortfall + ringing the doorbell here
        // restarts the stream so HID input flows indefinitely.
        self.top_up_hid_endpoints();
        dispatched
    }

    /// Re-post interrupt-IN TDs for any HID endpoint whose outstanding count has
    /// fallen below `HID_TD_RING`, restoring full multi-buffer depth. Idempotent
    /// and cheap when the depth is already full (the common case → no work). See
    /// the INDEFINITE-FLOW GUARD note in [`Self::service_hid_reports`].
    fn top_up_hid_endpoints(&mut self) {
        for slot_id in 1..=self.device_slots.len() as u8 {
            // Snapshot each endpoint's shortfall under an immutable borrow.
            let shortfalls: Vec<(u8, u32, u64, u64)> =
                match self.device_slots[(slot_id - 1) as usize].as_ref() {
                    Some(slot) => slot
                        .hid_interfaces
                        .iter()
                        .filter(|i| i.outstanding_tds < HID_TD_RING)
                        .map(|i| {
                            (
                                i.ep.ep_index,
                                i.ep.max_packet_size.max(8) as u32,
                                i.report_buf.phys,
                                HID_TD_RING - i.outstanding_tds,
                            )
                        })
                        .collect(),
                    None => Vec::new(),
                };
            for (ep_index, len, phys_base, missing) in shortfalls {
                // Post `missing` TDs into the report slots that follow the
                // current write head (drain_seq + outstanding), so re-fills don't
                // collide with an in-flight TD's buffer. Each TD lands
                // HID_TD_RING positions on in the FIFO ring, preserving the
                // slot↔order correspondence.
                let write_head = self.next_topup_slot(slot_id, ep_index);
                let mut posted = 0u64;
                for j in 0..missing {
                    let slot_off = (write_head.wrapping_add(j) % HID_TD_RING) * HID_SLOT_SIZE;
                    if self
                        .submit_interrupt_in(slot_id, ep_index, phys_base + slot_off, len)
                        .is_ok()
                    {
                        posted += 1;
                    } else {
                        break; // ring full — already at depth, stop topping up
                    }
                }
                if posted > 0 {
                    if let Some(slot) = self.device_slots[(slot_id - 1) as usize].as_mut() {
                        if let Some(iface) = slot
                            .hid_interfaces
                            .iter_mut()
                            .find(|i| i.ep.ep_index == ep_index)
                        {
                            iface.outstanding_tds =
                                (iface.outstanding_tds + posted).min(HID_TD_RING);
                        }
                    }
                }
            }
        }
    }

    /// Next report-slot index to use when topping up an endpoint. Uses the
    /// endpoint's `drain_seq + outstanding_tds` so re-fills queue into the slots
    /// AHEAD of both the read head (drain_seq) and the live TDs — keeping read
    /// and write heads from aliasing the same report buffer.
    fn next_topup_slot(&self, slot_id: u8, ep_index: u8) -> u64 {
        self.device_slots[(slot_id - 1) as usize]
            .as_ref()
            .and_then(|s| s.hid_interfaces.iter().find(|i| i.ep.ep_index == ep_index))
            .map(|i| i.drain_seq.wrapping_add(i.outstanding_tds))
            .unwrap_or(0)
    }

    /// Address → descriptors → SET_CONFIGURATION → HID interrupt EP → one report poll.
    pub fn bring_up_hid_keyboard(
        &mut self,
        slot_id: u8,
        vendor: u16,
        product: u16,
    ) -> Result<(), XhciError> {
        self.deferred_events.clear();
        // LowSpeed devices (≈always boot keyboards) CANNOT deliver a config
        // descriptor: it's always a multi-packet control-IN (≥9 bytes at MPS0=8),
        // and on the Athena's AMD xHC that read times out AND leaves EP0 *Halted*
        // (iron T1414: `config header` -> timeout -> `out before cfg: ep0=Halted`
        // -> SET_CONFIGURATION then times out on the halted EP0). So DON'T attempt
        // it — go straight to the boot-keyboard fallback while EP0 is still
        // healthy. A USB HID boot keyboard needs no descriptor parsing (HID 1.11
        // §7.2 / the BIOS boot protocol): interface 0, one interrupt-IN endpoint,
        // fixed 8-byte boot report. The fallback re-recovers EP0 right before
        // SET_CONFIGURATION so it runs as the FIRST post-recovery transfer (the
        // one this flaky EP0 reliably completes).
        let is_ls = self
            .device_slots
            .get((slot_id - 1) as usize)
            .and_then(|s| s.as_ref())
            .map(|s| matches!(s.speed, PortSpeed::LowSpeed))
            .unwrap_or(false);
        if is_ls {
            crate::serial_println!(
                "[xhci] HID: LowSpeed slot {} — boot-keyboard fallback (skip EP0-wedging config read)",
                slot_id
            );
            return self.bring_up_hid_boot_fallback(slot_id, vendor, product);
        }
        self.bring_up_hid_from_config(slot_id, vendor, product)
    }

    /// Normal HID bring-up: read the configuration descriptor and configure every
    /// HID interface it declares. Split out from [`Self::bring_up_hid_keyboard`]
    /// so a LowSpeed device whose EP0 cannot complete the (always multi-packet)
    /// config-descriptor read falls through to the boot-keyboard fallback.
    fn bring_up_hid_from_config(
        &mut self,
        slot_id: u8,
        vendor: u16,
        product: u16,
    ) -> Result<(), XhciError> {
        crate::serial_println!("[xhci] HID: fetching configuration descriptor...");
        crate::serial_println!("[xhci] HID: config header...");
        // Gentle stall-retry on every config read: a LowSpeed device that stalls
        // a multi-packet control-IN (Athena port-2 keyboard) often answers the
        // re-issue, so a transient stall no longer abandons enumeration here.
        let head = self.get_descriptor_retry(slot_id, 2, 0, 9)?;
        if head.len() < 9 {
            return Err(XhciError::InvalidState);
        }
        let total = u16::from_le_bytes([head[2], head[3]]) as usize;
        let want = total.min(512).max(9) as u16;
        crate::serial_println!("[xhci] HID: wTotalLength={} fetching {} bytes", total, want);
        let config = if want > 9 {
            self.get_descriptor_retry(slot_id, 2, 0, want)?
        } else {
            head
        };
        if config.len() < 9 {
            return Err(XhciError::InvalidState);
        }
        let total = u16::from_le_bytes([config[2], config[3]]) as usize;
        if total > config.len() {
            let full = self.get_descriptor_retry(slot_id, 2, 0, total.min(512) as u16)?;
            return self.bring_up_hid_keyboard_with_config(slot_id, vendor, product, &full);
        }
        self.bring_up_hid_keyboard_with_config(slot_id, vendor, product, &config)
    }

    /// Bring up a USB Mass Storage (BOT) device: fetch its configuration
    /// descriptor, configure both bulk endpoints, and SET_CONFIGURATION. After
    /// this the slot's device context has live bulk IN/OUT endpoints, so
    /// `usb_msc::probe_all_msc` can run INQUIRY / READ_CAPACITY / READ(10).
    pub fn bring_up_msc(
        &mut self,
        slot_id: u8,
        _vendor: u16,
        _product: u16,
    ) -> Result<(), XhciError> {
        self.deferred_events.clear();
        let head = self.get_descriptor(slot_id, 2, 0, 9)?;
        if head.len() < 9 {
            return Err(XhciError::InvalidState);
        }
        let total = u16::from_le_bytes([head[2], head[3]]) as usize;
        let want = total.min(512).max(9) as u16;
        let config = if want > 9 {
            self.get_descriptor(slot_id, 2, 0, want)?
        } else {
            head
        };
        let msc =
            crate::xhci_desc::find_msc_bulk(&config).ok_or(XhciError::EndpointNotConfigured)?;
        let config_value = config.get(5).copied().filter(|&v| v != 0).unwrap_or(1);

        // USB 3.x SuperSpeed handling distinct from USB2 (MasterChecklist 2.1):
        // SS+ links carry a Max Burst Size in each endpoint's SS Companion
        // descriptor (>0 only on SS); USB2 has none (burst 0). Log the link
        // speed + parsed burst so the distinction is visible at boot.
        let idx = (slot_id - 1) as usize;
        let speed = self.device_slots[idx]
            .as_ref()
            .map(|s| s.speed)
            .unwrap_or(PortSpeed::Undefined);
        let is_superspeed = matches!(speed, PortSpeed::SuperSpeed | PortSpeed::SuperSpeedPlus);
        crate::serial_println!(
            "[xhci] MSC: bulk_in={:#x}(idx{}) bulk_out={:#x}(idx{}) iface={} cfg={} speed={:?} burst[in={},out={}] mode={}",
            msc.in_ep_address,
            msc.in_ep_index,
            msc.out_ep_address,
            msc.out_ep_index,
            msc.interface_number,
            config_value,
            speed,
            msc.in_max_burst,
            msc.out_max_burst,
            if is_superspeed { "SuperSpeed(burst)" } else { "USB2(no-burst)" },
        );

        // Configure Endpoint must precede USB SET_CONFIGURATION (xHCI §4.3.5).
        let input_phys = {
            let slot = self.device_slots[idx]
                .as_mut()
                .ok_or(XhciError::InvalidSlot)?;
            slot.prepare_configure_msc_bulk(
                config_value,
                msc.interface_number,
                msc.in_ep_index,
                msc.in_max_packet,
                msc.in_max_burst,
                msc.out_ep_index,
                msc.out_max_packet,
                msc.out_max_burst,
            )?;
            slot.input_ctx.phys
        };
        self.configure_endpoint_with_ptr(slot_id, input_phys)?;
        // set_configuration rewinds EP0 internally (QEMU only).
        self.set_configuration(slot_id, config_value)?;
        crate::serial_println!(
            "[xhci] MSC bring-up OK: slot={} bulk endpoints configured",
            slot_id
        );
        Ok(())
    }

    /// Bring up a USB Audio Class device: configure its isochronous OUT endpoint,
    /// SET_CONFIGURATION + SET_INTERFACE(alt) to arm the stream, then push one
    /// service interval of silence to prove the isochronous DATA path completes.
    /// MasterChecklist Phase 2.6/7 — this lands the isoch TRANSPORT; a steady
    /// PCM stream layers a continuous TD ring on top of the same path.
    pub fn bring_up_audio(&mut self, slot_id: u8) -> Result<(), XhciError> {
        self.deferred_events.clear();
        // Two-step config-descriptor fetch (length prefix first), like MSC.
        let head = self.get_descriptor(slot_id, 2, 0, 9)?;
        if head.len() < 9 {
            return Err(XhciError::InvalidState);
        }
        let total = u16::from_le_bytes([head[2], head[3]]) as usize;
        let want = total.min(512).max(9) as u16;
        let config = if want > 9 {
            self.get_descriptor(slot_id, 2, 0, want)?
        } else {
            head
        };
        let uac = crate::usb_audio::parse_config(&config);
        let stream = uac
            .playback_stream()
            .or_else(|| uac.streams.first())
            .ok_or(XhciError::EndpointNotConfigured)?;
        let config_value = config.get(5).copied().filter(|&v| v != 0).unwrap_or(1);
        let epnum = stream.endpoint_addr & 0x0F;
        let dir_in = stream.endpoint_addr & 0x80 != 0;
        let dci = epnum * 2 + if dir_in { 1 } else { 0 };
        let ep_index = dci.saturating_sub(1);
        let mps = stream.bytes_per_interval.max(8);
        let interface = stream.interface;
        let alt = stream.alt_setting;
        // Full-speed audio: 1 ms service interval → xHCI Interval 3 (2^3 × 125µs).
        let interval = 3u8;

        crate::serial_println!(
            "[usb-audio] bring-up slot {}: iface={} alt={} isoch_ep={:#x}(idx{}) mps={} cfg={}",
            slot_id,
            interface,
            alt,
            stream.endpoint_addr,
            ep_index,
            mps,
            config_value
        );

        // Configure Endpoint (isoch OUT) must precede SET_CONFIGURATION (§4.3.5).
        let input_phys = {
            let idx = (slot_id - 1) as usize;
            let slot = self.device_slots[idx]
                .as_mut()
                .ok_or(XhciError::InvalidSlot)?;
            slot.prepare_configure_isoch_out(
                config_value,
                interface,
                alt,
                ep_index,
                mps,
                interval,
            )?;
            slot.input_ctx.phys
        };
        self.configure_endpoint_with_ptr(slot_id, input_phys)?;
        self.set_configuration(slot_id, config_value)?;
        // Activate the streaming alt setting (alt 0 = zero bandwidth → no data).
        if let Err(e) = self.set_interface(slot_id, interface, alt) {
            crate::serial_println!(
                "[usb-audio] SET_INTERFACE(iface {} alt {}) failed: {:?} (continuing)",
                interface,
                alt,
                e
            );
        }

        // Stream a 440 Hz test tone through a multi-buffered isoch ring (the
        // tone stands in for the RaeAudio mixer output). The ring-prime depth
        // streams cleanly; sustaining the stream past it needs isoch
        // endpoint-restart-on-underrun handling (an isoch EP STOPS when its ring
        // drains — unlike interrupt EPs — so re-arming a stopped EP is more than
        // a doorbell; tracked follow-up, may also be a QEMU isoch-model limit).
        let frames = 32usize;
        match self.play_test_tone(slot_id, ep_index, mps, frames) {
            Ok(drained) => crate::serial_println!(
                "[usb-audio] streamed {}/{} frames ({} ms) of 440Hz tone to the DAC — {}",
                drained,
                frames,
                drained,
                if drained == frames {
                    "continuous playback OK"
                } else {
                    "multi-buffered prime drained; sustained stream pends isoch underrun-restart"
                }
            ),
            Err(e) => crate::serial_println!("[usb-audio] tone stream failed: {:?}", e),
        }
        Ok(())
    }

    /// Stream `total_frames` × 1 ms of a 440 Hz test tone to an armed isochronous
    /// OUT endpoint, in REFILLED batches — the essence of continuous playback:
    /// keep the TD ring fed across service intervals rather than firing one TD.
    /// Returns the number of frames the DAC drained (transfer events observed).
    /// MasterChecklist Phase 2.6/7. A real player swaps `fill_square_tone` for
    /// the RaeAudio mixer and runs this loop on the SCHED_GAME audio thread.
    fn play_test_tone(
        &mut self,
        slot_id: u8,
        ep_index: u8,
        mps: u16,
        total_frames: usize,
    ) -> Result<usize, XhciError> {
        let mps_u = mps as usize;
        const DEPTH: usize = 4; // isoch TDs kept in flight (multi-buffering)
        if mps_u == 0 || mps_u * DEPTH > 4096 {
            return Err(XhciError::InvalidState);
        }
        let page = alloc_dma_page()?;
        let page_virt = page.virt;
        let page_phys = page.phys;
        let mut phase = 0u32;
        let fill = |i: usize, phase: u32| -> u32 {
            let off = (i * mps_u) as u64;
            let b = unsafe { core::slice::from_raw_parts_mut((page_virt + off) as *mut u8, mps_u) };
            fill_square_tone(b, phase)
        };

        // Prime DEPTH TDs so the xHC ALWAYS has the next frame's TD ready.
        // Isochronous has no flow control: an empty ring at a frame boundary is a
        // Missed Service Error (the gap that dropped all but the first TD when
        // submitting one-at-a-time). Multi-buffering keeps the pipe full.
        let mut submitted = 0usize;
        for i in 0..DEPTH.min(total_frames) {
            phase = fill(i, phase);
            if self
                .submit_isoch_transfer(
                    slot_id,
                    ep_index,
                    page_phys + (i * mps_u) as u64,
                    mps as u32,
                    0,
                )
                .is_err()
            {
                break;
            }
            submitted += 1;
        }
        // Steady state: each completion frees a buffer → refill + resubmit it,
        // holding DEPTH TDs in flight until every frame has been sent + drained.
        let mut drained = 0usize;
        while drained < submitted {
            // FORWARD-LOOKING (MasterChecklist Phase 2.6/7): this one-shot boot
            // test-tone simply aborts on the first error. When USB isoch becomes
            // a real RaeAudio sink, a software stall that misses a frame boundary
            // raises a Missed Service Error (TrbCompletionCode::MissedService=23)
            // and HALTS the isoch ring — recovery then needs Reset Endpoint ->
            // Set TR Dequeue Pointer to restart the stream (same shape as the EP0
            // stall recovery), or audio dies permanently after a single stutter.
            // Not built here: the boot test never streams long enough to miss a
            // frame, and the production audio path is HDA, not USB isoch.
            if self.wait_for_transfer(0).is_err() {
                break;
            }
            drained += 1;
            if submitted < total_frames {
                let i = submitted % DEPTH;
                phase = fill(i, phase);
                if self
                    .submit_isoch_transfer(
                        slot_id,
                        ep_index,
                        page_phys + (i * mps_u) as u64,
                        mps as u32,
                        0,
                    )
                    .is_ok()
                {
                    submitted += 1;
                }
            }
        }
        // Free the scratch page ONLY if every submitted TD completed
        // (drained == submitted, the normal loop exit) — then the xHC holds no
        // reference into it. On the error break path drained < submitted, i.e.
        // isoch TDs are still in flight pointing into the page, so leak it rather
        // than free a frame a pending TD may still DMA into (same rule as the
        // control-transfer error paths).
        if drained == submitted {
            page.free();
        }
        Ok(drained)
    }

    fn bring_up_hid_keyboard_with_config(
        &mut self,
        slot_id: u8,
        vendor: u16,
        product: u16,
        config: &[u8],
    ) -> Result<(), XhciError> {
        // Bind EVERY HID interface, not just the first. A composite peripheral
        // (the Athena Razer 1532:0098 is a mouse whose interface 0 is a
        // keyboard/media interface, with the pointer on a later interface) would
        // otherwise have its mouse interface missed → dead cursor.
        let mut interfaces = crate::xhci_desc::find_all_hid_interfaces(config);
        if interfaces.is_empty() {
            return Err(XhciError::EndpointNotConfigured);
        }
        // Ascending ep_index: each incremental Configure Endpoint command only
        // ever RAISES the slot's context-entries count, so a later command can't
        // truncate an endpoint an earlier one added (xHCI §4.6.6 leaves
        // unreferenced endpoints intact).
        interfaces.sort_by_key(|e| e.ep_index);

        let config_value = config.get(5).copied().filter(|&v| v != 0).unwrap_or(1);

        // 1. Configure each interrupt endpoint (xHCI Configure Endpoint, one per
        //    interface — incremental). Must precede USB SET_CONFIGURATION.
        for ep in &interfaces {
            let is_mouse = ep.protocol == crate::xhci_desc::HID_PROTO_MOUSE;
            crate::serial_println!(
                "[xhci] HID interrupt ep_index={} addr={:#x} mps={} interval={} iface={} ({})",
                ep.ep_index,
                ep.ep_address,
                ep.max_packet_size,
                ep.interval,
                ep.interface_number,
                if is_mouse { "mouse" } else { "keyboard/other" }
            );
            self.configure_hid_interrupt(slot_id, config_value, *ep)?;
        }
        // 2. USB SET_CONFIGURATION once — device-wide, enables every interface.
        crate::serial_println!("[xhci] HID: SET_CONFIGURATION({})...", config_value);
        self.set_configuration(slot_id, config_value)?;

        // 3. Per interface: pick boot vs report protocol, register, store, arm.
        for ep in &interfaces {
            let hid_ep = *ep;
            let is_mouse = hid_ep.protocol == crate::xhci_desc::HID_PROTO_MOUSE;
            // Boot-capable = Boot Interface Subclass (1) AND a boot protocol
            // (1=keyboard, 2=mouse): only these accept SET_PROTOCOL(boot)/SET_IDLE
            // and emit the fixed boot-report layout. Everything else stays in
            // REPORT protocol and is decoded via the report descriptor (`raehid`,
            // Redox's `hidreport`) — sending SET_PROTOCOL(boot) to a non-boot
            // interface STALLs EP0.
            let boot_capable = hid_ep.subclass == 1
                && (hid_ep.protocol == crate::xhci_desc::HID_PROTO_KEYBOARD
                    || hid_ep.protocol == crate::xhci_desc::HID_PROTO_MOUSE);
            let iface = hid_ep.interface_number as u16;

            let mut parsed: Option<raehid::HidDevice> = None;
            if boot_capable {
                // SET_PROTOCOL(boot=0): bmRequestType=0x21 (host→iface, class), bRequest=0x0B.
                if let Err(e) = self.control_out_nodata(slot_id, 0x21, 0x0B, 0x0000, iface) {
                    crate::serial_println!(
                        "[xhci] HID: SET_PROTOCOL(boot) iface {} skipped: {:?}",
                        iface,
                        e
                    );
                }
                if let Err(e) = self.control_out_nodata(slot_id, 0x21, 0x0A, 0x0000, iface) {
                    crate::serial_println!(
                        "[xhci] HID: SET_IDLE(0) iface {} skipped: {:?}",
                        iface,
                        e
                    );
                }
            } else if hid_ep.report_desc_len > 0 {
                // Report-protocol interface: fetch + parse its report descriptor.
                // Best-effort — any failure falls back to boot-layout parsing.
                match self.get_hid_report_descriptor(slot_id, iface, hid_ep.report_desc_len) {
                    Ok(bytes) => match raehid::HidDevice::parse(&bytes) {
                        Some(dev) => {
                            crate::serial_println!(
                                "[xhci] HID: report-protocol iface {} — parsed {}-byte report descriptor, kind={:?}",
                                iface,
                                bytes.len(),
                                dev.kind()
                            );
                            parsed = Some(dev);
                        }
                        None => crate::serial_println!(
                            "[xhci] HID: iface {} report descriptor ({} bytes) did not parse — boot fallback",
                            iface,
                            bytes.len()
                        ),
                    },
                    Err(e) => crate::serial_println!(
                        "[xhci] HID: iface {} report-descriptor fetch failed ({:?}) — boot fallback",
                        iface,
                        e
                    ),
                }
                let _ = self.control_out_nodata(slot_id, 0x21, 0x0A, 0x0000, iface);
            }

            let kind_is_mouse = match parsed.as_ref() {
                Some(dev) => dev.kind() == raehid::HidKind::Mouse,
                None => is_mouse,
            };

            let report_page = alloc_dma_page()?;
            let input_id = {
                use crate::input::{InputDeviceInfo, InputDeviceType};
                use alloc::string::String;
                let (name, device_type) = if kind_is_mouse {
                    ("USB HID Mouse", InputDeviceType::Mouse)
                } else {
                    ("USB HID Keyboard", InputDeviceType::Keyboard)
                };
                crate::input::register_device(InputDeviceInfo {
                    id: 0,
                    name: String::from(name),
                    vendor_id: vendor,
                    product_id: product,
                    device_type,
                    serial: None,
                })
            };

            if let Some(ref mut slot) = self.device_slots[(slot_id - 1) as usize] {
                slot.hid_interfaces.push(HidInterface {
                    ep: hid_ep,
                    report_buf: report_page,
                    input_device_id: input_id,
                    hid_device: parsed,
                    drain_seq: 0,
                    outstanding_tds: 0,
                });
            }
            self.sync_ep_enqueue_from_hardware(slot_id, hid_ep.ep_index);
        }

        crate::serial_println!(
            "[xhci] HID: {} interface(s) configured and armed. Waiting for user input.",
            interfaces.len()
        );
        Ok(())
    }

    /// Bring up a HID boot keyboard WITHOUT reading its configuration descriptor.
    ///
    /// Used only when the normal descriptor-driven path fails on a LowSpeed device
    /// (the Athena port-2 keyboard: EP0 completes 1-packet control-IN but
    /// STALLs/times-out the always-multi-packet config descriptor — an AMD-xHC
    /// LowSpeed quirk that no descriptor read can defeat). The USB HID boot
    /// protocol exists precisely for minimal drivers (BIOS) that cannot parse
    /// descriptors: a boot keyboard is interface 0 with a single interrupt-IN
    /// endpoint and streams the fixed 8-byte boot report (HID 1.11 §7.2,
    /// Appendix B). We synthesize that standard layout — EP1 IN, MPS 8, ~8 ms —
    /// and drive it using ONLY the transfers proven to work on the device:
    /// Configure Endpoint (a command, no EP0 transfer), SET_CONFIGURATION /
    /// SET_PROTOCOL(boot) / SET_IDLE (0-packet control-OUT), and 8-byte
    /// interrupt-IN reports (1 packet). No multi-packet control-IN anywhere.
    fn bring_up_hid_boot_fallback(
        &mut self,
        slot_id: u8,
        vendor: u16,
        product: u16,
    ) -> Result<(), XhciError> {
        const CONFIG_VALUE: u8 = 1; // boot keyboards expose configuration 1
                                    // EP1 IN: address 0x81 ⇒ DCI 3 ⇒ endpoints[] index 2. The overwhelming
                                    // convention for boot keyboards (and what BIOS assumes). bInterval 8 ⇒
                                    // ~8 ms poll for LS (prepare_configure_hid maps it to the legal exponent).
        let boot_ep = crate::xhci_desc::HidInterruptEndpoint {
            ep_index: 2,
            ep_address: 0x81,
            interface_number: 0,
            alternate_setting: 0,
            max_packet_size: 8,
            interval: 8,
            protocol: crate::xhci_desc::HID_PROTO_KEYBOARD,
            subclass: 1, // Boot Interface Subclass
            report_desc_len: 0,
        };
        crate::serial_println!(
            "[xhci] HID boot fallback slot {}: synthesizing boot keyboard (iface 0, EP1 IN, mps 8)",
            slot_id
        );

        // 1. Configure the interrupt-IN endpoint (xHCI Configure Endpoint — a
        //    command ring op, not an EP0 transfer). Must precede SET_CONFIGURATION.
        self.configure_hid_interrupt(slot_id, CONFIG_VALUE, boot_ep)?;

        // 2. USB SET_CONFIGURATION (0-packet control-OUT) — enables the interface.
        //    CRITICAL (iron T1414): this LowSpeed EP0 reliably completes the FIRST
        //    transfer after a recover_ep0 but wedges (Halt/Timeout) on the next.
        //    The earlier device-descriptor read already consumed a post-recovery
        //    transfer, so recover EP0 again HERE to make SET_CONFIGURATION the
        //    first post-recovery transfer — otherwise it times out on a halted EP0.
        let _ = self.recover_ep0(slot_id);
        self.set_configuration(slot_id, CONFIG_VALUE)?;

        // 3. SET_PROTOCOL(boot=0) + SET_IDLE(0) on interface 0 (both 0-packet
        //    control-OUT). Best-effort, and DELIBERATELY NOT preceded by a
        //    recover_ep0: iron T1534 showed that extra recover_ep0 churn on this
        //    slot's EP0 (esp. an illegal Reset Endpoint on a non-Halted EP0 →
        //    ContextStateError) destabilizes the shared controller and breaks the
        //    NEXT device's enumeration — the Razer mouse on slot 3 regressed to
        //    "EP0 8-byte read FAILED". A boot keyboard left in report protocol
        //    still streams the 8-byte boot layout (its report 0), and the
        //    interrupt EP (armed via the command above) is independent of EP0, so
        //    these two are pure best-effort — never recover for them.
        if let Err(e) = self.control_out_nodata(slot_id, 0x21, 0x0B, 0x0000, 0) {
            crate::serial_println!(
                "[xhci] HID boot fallback: SET_PROTOCOL(boot) skipped: {:?}",
                e
            );
        }
        if let Err(e) = self.control_out_nodata(slot_id, 0x21, 0x0A, 0x0000, 0) {
            crate::serial_println!("[xhci] HID boot fallback: SET_IDLE(0) skipped: {:?}", e);
        }

        // 4. Register the input device + record the interface so arm_hid_interrupts
        //    submits the interrupt-IN TD and service_hid_reports routes its reports
        //    through the boot-protocol keyboard path (hid_device == None).
        let report_page = alloc_dma_page()?;
        let input_id = {
            use crate::input::{InputDeviceInfo, InputDeviceType};
            use alloc::string::String;
            crate::input::register_device(InputDeviceInfo {
                id: 0,
                name: String::from("USB HID Keyboard (boot)"),
                vendor_id: vendor,
                product_id: product,
                device_type: InputDeviceType::Keyboard,
                serial: None,
            })
        };
        if let Some(ref mut slot) = self.device_slots[(slot_id - 1) as usize] {
            slot.hid_interfaces.push(HidInterface {
                ep: boot_ep,
                report_buf: report_page,
                input_device_id: input_id,
                hid_device: None,
                drain_seq: 0,
                outstanding_tds: 0,
            });
        }
        self.sync_ep_enqueue_from_hardware(slot_id, boot_ep.ep_index);

        crate::serial_println!(
            "[xhci] HID boot fallback slot {}: keyboard armed (config-descriptor-free path)",
            slot_id
        );
        Ok(())
    }

    pub fn enable_slot(&mut self) -> Result<u8, XhciError> {
        let cmd_ring = self.command_ring.as_mut().ok_or(XhciError::NotReady)?;
        cmd_ring.enable_slot()?;
        self.ring_doorbell_host();
        let event = self.wait_for_command(500_000)?;
        let code = TrbCompletionCode::from_u8(((event.status >> 24) & 0xFF) as u8);
        if code != TrbCompletionCode::Success {
            return Err(XhciError::CommandFailed(code));
        }
        let slot_id = ((event.control >> 24) & 0xFF) as u8;
        if slot_id == 0 {
            return Err(XhciError::NoSlots);
        }
        Ok(slot_id)
    }

    pub fn disable_slot(&mut self, slot_id: u8) -> Result<(), XhciError> {
        if slot_id == 0 || slot_id as usize > self.device_slots.len() {
            return Err(XhciError::InvalidSlot);
        }
        let cmd_ring = self.command_ring.as_mut().ok_or(XhciError::NotReady)?;
        cmd_ring.disable_slot(slot_id)?;
        self.ring_doorbell_host();
        self.device_slots[(slot_id - 1) as usize] = None;
        Ok(())
    }

    pub fn address_device(
        &mut self,
        slot_id: u8,
        port_id: u8,
        speed: PortSpeed,
        bsr: bool,
    ) -> Result<(), XhciError> {
        self.address_device_routed(slot_id, port_id, 0, speed, None, bsr)
    }

    /// Address a device that may sit behind one or more hubs. `root_port` is the
    /// top-level root-hub port the chain hangs off; `route_string` encodes the
    /// per-tier downstream port path; `parent` (when present) supplies the TT
    /// info for low/full-speed devices behind a high-speed hub.
    pub fn address_device_routed(
        &mut self,
        slot_id: u8,
        root_port: u8,
        route_string: u32,
        speed: PortSpeed,
        parent: Option<ParentHub>,
        bsr: bool,
    ) -> Result<(), XhciError> {
        if slot_id == 0 || slot_id as usize > self.device_slots.len() {
            return Err(XhciError::InvalidSlot);
        }
        let mut slot = DeviceSlot::new(slot_id, root_port, speed)?;
        slot.setup_address_context_routed(route_string, parent, self.context_size_64);
        // CSZ=1 controllers read 64-byte context entries — rewrite the
        // compact image to spec stride or the xHC sees garbage and fails
        // the command with ParameterError (photographed on Athena).
        if self.context_size_64 {
            slot.expand_input_context_csz64();
        }

        if let Some(ref mut dcbaa) = self.dcbaa {
            dcbaa.set_device_context(slot_id, slot.output_ctx.phys);
        }

        let input_ctx_ptr = slot.input_ctx.phys;
        let cmd_ring = self.command_ring.as_mut().ok_or(XhciError::NotReady)?;
        cmd_ring.address_device(slot_id, input_ctx_ptr, bsr)?;
        self.ring_doorbell_host();

        let event = self.wait_for_command(500_000)?;
        let code = TrbCompletionCode::from_u8(((event.status >> 24) & 0xFF) as u8);
        if code != TrbCompletionCode::Success {
            return Err(XhciError::CommandFailed(code));
        }

        let out_dcs = slot
            .output_endpoint(self.context_size_64, 1)
            .dequeue_cycle_state();
        if let Some(ref mut ep0_ring) = slot.transfer_rings[0] {
            ep0_ring.cycle_state = out_dcs;
        }

        slot.addressed = true;
        let usb_addr = unsafe {
            let out = &*(slot.output_ctx.virt as *const DeviceContext);
            out.slot.usb_device_address()
        };
        crate::serial_println!(
            "[xhci] slot {} USB address {} (output slot state {:?})",
            slot_id,
            usb_addr,
            unsafe {
                let out = &*(slot.output_ctx.virt as *const DeviceContext);
                out.slot.slot_state()
            }
        );
        self.device_slots[(slot_id - 1) as usize] = Some(slot);
        if !bsr {
            // USB 2.0 §9.2.6.3: 2 ms SET_ADDRESS recovery before the first
            // request to the new address. Skipping it (fine on QEMU) made
            // real devices stall/babble the GET_DESCRIPTOR that follows.
            crate::hpet::spin_until_us(2_000, || false);
        }
        Ok(())
    }

    /// Recover a halted EP0 after a STALL/babble. A control endpoint that
    /// halts rejects EVERY subsequent transfer on the slot, so one stalled
    /// GET_DESCRIPTOR otherwise turns the device permanently dead (Athena
    /// port 4). Sequence per xHCI §4.6.8: Reset Endpoint (DCI 1) to leave
    /// the Halted state, reset the software ring, then Set TR Dequeue to
    /// re-arm the hardware at the ring base with DCS=1.
    pub fn recover_ep0(&mut self, slot_id: u8) -> Result<(), XhciError> {
        let ctx64 = self.context_size_64;
        let idx = (slot_id - 1) as usize;
        // DIAGNOSTIC (iron, 2026-06-15): capture EP0 state before recovery. If it
        // is NOT Halted, the STALL never halted EP0 — recovery is a no-op and the
        // post-recovery transfer timeout (Athena port-2 keyboard, bootlog T1707) is
        // a DIFFERENT bug (transfer ring / doorbell, not endpoint state). This is
        // the ground truth that earlier blind fixes lacked.
        let before = self
            .device_slots
            .get(idx)
            .and_then(|s| s.as_ref())
            .map(|s| {
                let ep0 = s.output_endpoint(ctx64, 1);
                (ep0.ep_state(), ep0.tr_dequeue_pointer())
            });
        {
            let cmd_ring = self.command_ring.as_mut().ok_or(XhciError::NotReady)?;
            cmd_ring.reset_endpoint(slot_id, 1, false)?;
        }
        self.ring_doorbell_host();
        // ContextStateError just means EP0 wasn't actually halted — the Set
        // TR Dequeue below is then a harmless re-arm; don't bail on it.
        let reset_evt = self.wait_for_command(500_000)?;
        let reset_code = TrbCompletionCode::from_u8(((reset_evt.status >> 24) & 0xFF) as u8);

        let ring_phys = {
            let slot = self
                .device_slots
                .get_mut(idx)
                .and_then(|s| s.as_mut())
                .ok_or(XhciError::InvalidSlot)?;
            // EP0's software ring lives at transfer_rings[0] (DCI 1).
            slot.reset_endpoint(0);
            slot.transfer_rings[0]
                .as_ref()
                .map(|r| r.page.phys)
                .ok_or(XhciError::EndpointNotConfigured)?
        };
        {
            let cmd_ring = self.command_ring.as_mut().ok_or(XhciError::NotReady)?;
            cmd_ring.set_tr_dequeue_pointer(slot_id, 1, ring_phys, true, 0)?;
        }
        self.ring_doorbell_host();
        let deq_evt = self.wait_for_command(500_000)?;
        let deq_code = TrbCompletionCode::from_u8(((deq_evt.status >> 24) & 0xFF) as u8);
        let after = self
            .device_slots
            .get(idx)
            .and_then(|s| s.as_ref())
            .map(|s| {
                let ep0 = s.output_endpoint(ctx64, 1);
                (ep0.ep_state(), ep0.tr_dequeue_pointer())
            });
        crate::serial_println!(
            "[xhci] recover_ep0 slot {}: EP0 before={:?} ResetEndpoint={:?} SetTRDeq={:?} EP0 after={:?} ring_phys={:#x}",
            slot_id, before, reset_code, deq_code, after, ring_phys
        );
        // Drop any events left over from the failed transfer(s). With the EP0
        // ring now reset to its base (Set TR Dequeue above) and the software ring
        // reset to match, stale deferred entries are meaningless to the NEXT
        // transfer and would only pollute its wait (Athena port-2 keyboard:
        // deferred events piled up across the stall→recover→retry cycle).
        self.deferred_events.clear();
        Ok(())
    }

    /// FullSpeed EP0 max-packet discovery (USB 2.0 §5.5.3). FS devices may
    /// use 8/16/32/64 for EP0 and there is no way to know before asking, so
    /// we address with the safe default (8), read JUST the first 8 descriptor
    /// bytes (legal at any MPS), and when bMaxPacketSize0 differs, update the
    /// endpoint context via Evaluate Context. Skipping this is exactly what
    /// babbled Athena's FullSpeed port-2 device: its 18-byte descriptor
    /// arrived in one 64-byte packet, exceeding the declared 8-byte MPS.
    pub fn ensure_fs_ep0_mps(&mut self, slot_id: u8) {
        let idx = (slot_id - 1) as usize;
        let (cur, speed) = match self.device_slots.get(idx).and_then(|s| s.as_ref()) {
            Some(s) => (s.max_packet_size_ep0, s.speed),
            None => return,
        };
        // HID-DIAG: surface the 8-byte EP0 read outcome. This is the FIRST
        // control transfer to a freshly-addressed device. If it STALLs even here
        // (esp. the LowSpeed port-2 keyboard), the device does NO control
        // transfers at all — the problem is upstream of descriptor length
        // (EP0 ring / addressing / device readiness), not a 64-vs-8 MPS issue.
        let d8 = match self.get_descriptor(slot_id, 1, 0, 8) {
            Ok(d) if d.len() >= 8 => {
                crate::serial_println!(
                    "[hid-diag] slot {} ({:?}) EP0 8-byte read OK: {:02x?}",
                    slot_id,
                    speed,
                    &d[..8]
                );
                d
            }
            first => {
                crate::serial_println!(
                    "[hid-diag] slot {} ({:?}) EP0 8-byte read FAILED ({:?}) — recover+retry",
                    slot_id,
                    speed,
                    first.err()
                );
                let _ = self.recover_ep0(slot_id);
                crate::hpet::spin_until_us(2_000, || false);
                match self.get_descriptor(slot_id, 1, 0, 8) {
                    Ok(d) if d.len() >= 8 => d,
                    second => {
                        crate::serial_println!(
                            "[hid-diag] slot {} ({:?}) EP0 8-byte read STILL failing ({:?}) — device does no control transfers",
                            slot_id,
                            speed,
                            second.err()
                        );
                        return;
                    }
                }
            }
        };
        let dev_mps = d8[7] as u16;
        if !matches!(dev_mps, 8 | 16 | 32 | 64) || dev_mps == cur {
            return;
        }
        crate::serial_println!(
            "[xhci] slot {} FS EP0 max packet {} -> {} (Evaluate Context)",
            slot_id,
            cur,
            dev_mps,
        );
        let input_ptr = {
            let ctx64 = self.context_size_64;
            let Some(slot) = self.device_slots.get_mut(idx).and_then(|s| s.as_mut()) else {
                return;
            };
            slot.prepare_evaluate_ep0_mps(dev_mps, ctx64)
        };
        {
            let Some(cmd_ring) = self.command_ring.as_mut() else {
                return;
            };
            if cmd_ring.evaluate_context(slot_id, input_ptr).is_err() {
                return;
            }
        }
        self.ring_doorbell_host();
        match self.wait_for_command(500_000) {
            Ok(ev) => {
                let code = TrbCompletionCode::from_u8(((ev.status >> 24) & 0xFF) as u8);
                if code != TrbCompletionCode::Success {
                    crate::serial_println!(
                        "[xhci] Evaluate Context (slot {} EP0 MPS) failed: {:?}",
                        slot_id,
                        code
                    );
                }
            }
            Err(e) => {
                crate::serial_println!("[xhci] Evaluate Context wait failed: {:?}", e);
            }
        }
    }

    /// GET_DESCRIPTOR(device) with one EP0-recovery retry. Real devices
    /// sometimes stall the very first request after reset/address (marginal
    /// timing, hub TT warm-up); without recovery that single stall ended the
    /// device's enumeration for good.
    pub fn get_device_descriptor_with_recovery(
        &mut self,
        slot_id: u8,
    ) -> Result<[u8; 18], XhciError> {
        let first = self.get_device_descriptor(slot_id);
        let e = match first {
            Ok(d) => return Ok(d),
            Err(e) => e,
        };
        crate::serial_println!(
            "[xhci] GET_DESCRIPTOR slot {} failed ({:?}) — EP0 halted, recovering first",
            slot_id,
            e
        );
        // CORRECTED ORDER (Athena port-2 keyboard, bootlogs T1737/T1707): a control
        // STALL HALTS EP0 (xHCI), and a HALTED endpoint IGNORES every doorbell until
        // a Reset Endpoint — so retrying the read on a halted EP just TIMES OUT. The
        // old code retried first and only recovered last, so the keyboard's EP0
        // stayed halted through every retry. Recover FIRST. `recover_ep0` is the
        // standard Reset Endpoint + Set TR Dequeue and is iron-PROVEN to SUCCEED on
        // AMD (the diagnostic showed EP0 Halted→Stopped, ResetEndpoint=Success) —
        // the old "recover_ep0 HCE-halts AMD" fear was about the per-transfer
        // reset_ep0_transfer_ring crutch, which is gone; this command path is safe.
        if let Err(re) = self.recover_ep0(slot_id) {
            crate::serial_println!("[xhci] EP0 recovery failed: {:?}", re);
        }
        crate::hpet::spin_until_us(20_000, || false);
        // After recovery, try the 8-BYTE HEADER FIRST. The Athena port-2 LowSpeed
        // keyboard answers an 8-byte GET_DESCRIPTOR (proven live by the [hid-diag]
        // read) but STALLS the full 18-byte one — bMaxPacketSize0=8 ⇒ 18 B is a
        // 3-packet control-IN it can't complete. Re-issuing the 18-byte read here
        // would just STALL AGAIN and re-halt EP0, so the next read times out on a
        // freshly-halted endpoint — exactly the T1707 wedge. The 8-byte header
        // carries bMaxPacketSize0 + bDeviceClass, all enumeration needs before the
        // (separately-transferred) config descriptor. `if let Ok` (not `?`) so a
        // genuine failure falls through to the 18-byte retry, not abort.
        if let Ok(d8) = self.get_descriptor(slot_id, 1, 0, 8) {
            if d8.len() >= 8 {
                let mut out = [0u8; 18];
                out[..8].copy_from_slice(&d8[..8]);
                crate::serial_println!(
                    "[xhci] slot {} descriptor OK after EP0 recovery (8-byte header, LS multi-packet quirk)",
                    slot_id
                );
                return Ok(out);
            }
        }
        // 8-byte also failed → the device's first stall may have been transient
        // timing that recovery fully cleared, so try the full 18-byte once.
        if let Ok(d) = self.get_device_descriptor(slot_id) {
            crate::serial_println!(
                "[xhci] slot {} full descriptor OK after EP0 recovery",
                slot_id
            );
            return Ok(d);
        }
        // TRUE LAST RESORT: a second heavy recovery + one more attempt pair.
        crate::serial_println!(
            "[xhci] slot {} still failing — second EP0 recovery + retry",
            slot_id
        );
        if let Err(re) = self.recover_ep0(slot_id) {
            crate::serial_println!("[xhci] EP0 recovery failed: {:?}", re);
        }
        crate::hpet::spin_until_us(10_000, || false);
        if let Ok(d8) = self.get_descriptor(slot_id, 1, 0, 8) {
            if d8.len() >= 8 {
                let mut out = [0u8; 18];
                out[..8].copy_from_slice(&d8[..8]);
                crate::serial_println!(
                    "[xhci] slot {} 8-byte header OK after second recovery",
                    slot_id
                );
                return Ok(out);
            }
        }
        self.get_device_descriptor(slot_id)
    }

    pub fn configure_endpoint(&mut self, slot_id: u8) -> Result<(), XhciError> {
        let idx = (slot_id - 1) as usize;
        let input_ctx_ptr = self.device_slots[idx]
            .as_ref()
            .ok_or(XhciError::InvalidSlot)?
            .input_ctx
            .phys;
        self.configure_endpoint_with_ptr(slot_id, input_ctx_ptr)
    }

    fn configure_endpoint_with_ptr(
        &mut self,
        slot_id: u8,
        input_ctx_ptr: u64,
    ) -> Result<(), XhciError> {
        let idx = (slot_id - 1) as usize;
        // Every caller passes the slot's own input_ctx, freshly built in the
        // compact 32-byte-entry layout — expand it for CSZ=1 controllers
        // (see expand_input_context_csz64).
        if self.context_size_64 {
            if let Some(slot) = self.device_slots[idx].as_ref() {
                if slot.input_ctx.phys == input_ctx_ptr {
                    slot.expand_input_context_csz64();
                }
            }
        }
        crate::serial_println!(
            "[xhci] ConfigureEndpoint slot={} input_ctx={:#x}",
            slot_id,
            input_ctx_ptr
        );
        let cmd_ring = self.command_ring.as_mut().ok_or(XhciError::NotReady)?;
        cmd_ring.configure_endpoint(slot_id, input_ctx_ptr, false)?;
        self.ring_doorbell_host();
        let event = self.wait_for_command(500_000)?;
        let code = TrbCompletionCode::from_u8(((event.status >> 24) & 0xFF) as u8);
        if code != TrbCompletionCode::Success {
            crate::serial_println!(
                "[xhci] ConfigureEndpoint failed: {:?} (input_ctx={:#x})",
                code,
                input_ctx_ptr
            );
            return Err(XhciError::CommandFailed(code));
        }

        if let Some(ref mut slot) = self.device_slots[idx] {
            slot.configured = true;
        }
        crate::serial_println!("[xhci] ConfigureEndpoint OK slot={}", slot_id);
        Ok(())
    }

    /// Set TR dequeue pointer for a non-EP0 endpoint (endpoint ID = DCI).
    pub fn reset_endpoint_by_index(&mut self, slot_id: u8, ep_index: u8) -> Result<(), XhciError> {
        let endpoint_id = self.doorbell_target(ep_index);
        let cmd_ring = self.command_ring.as_mut().ok_or(XhciError::NotReady)?;
        cmd_ring.reset_endpoint(slot_id, endpoint_id, false)?;
        self.ring_doorbell_host();
        let event = self.wait_for_command(500_000)?;
        let code = TrbCompletionCode::from_u8(((event.status >> 24) & 0xFF) as u8);
        if code != TrbCompletionCode::Success {
            return Err(XhciError::CommandFailed(code));
        }
        Ok(())
    }

    pub fn set_endpoint_tr_dequeue(
        &mut self,
        slot_id: u8,
        ep_index: u8,
        dequeue_ptr: u64,
        dcs: bool,
    ) -> Result<(), XhciError> {
        let endpoint_id = self.doorbell_target(ep_index);
        let cmd_ring = self.command_ring.as_mut().ok_or(XhciError::NotReady)?;
        cmd_ring.set_tr_dequeue_pointer(slot_id, endpoint_id, dequeue_ptr, dcs, 0)?;
        self.ring_doorbell_host();
        let event = self.wait_for_command(500_000)?;
        let code = TrbCompletionCode::from_u8(((event.status >> 24) & 0xFF) as u8);
        if code != TrbCompletionCode::Success {
            return Err(XhciError::CommandFailed(code));
        }
        Ok(())
    }

    pub fn reset_device(&mut self, slot_id: u8) -> Result<(), XhciError> {
        if slot_id == 0 || slot_id as usize > self.device_slots.len() {
            return Err(XhciError::InvalidSlot);
        }
        let cmd_ring = self.command_ring.as_mut().ok_or(XhciError::NotReady)?;
        cmd_ring.reset_device(slot_id)?;
        self.ring_doorbell_host();
        Ok(())
    }

    pub fn ring_doorbell_host(&self) {
        let db_offset = self.doorbell_base();
        self.write_reg32(db_offset, 0);
    }

    pub fn ring_doorbell_device(&self, slot_id: u8, target: u8, stream_id: u16) {
        let db_offset = self.doorbell_base() + (slot_id as u64) * 4;
        let value = (target as u32) | ((stream_id as u32) << 16);
        self.write_reg32(db_offset, value);
    }

    /// Advance the software dequeue index of the transfer ring that produced
    /// this Transfer Event. Without this the producer side is the only thing
    /// that ever moves: after `segment_size - 2` submissions `is_full()`
    /// reports a permanently-full ring and every later enqueue fails with
    /// RingFull — which killed any long bulk session (the bootlog FAT-chain
    /// walk dies after ~120 MSC sector reads) and would stop HID endpoints
    /// after the same number of reports.
    fn note_transfer_completion(&mut self, event: &Trb) {
        let slot_id = ((event.control >> 24) & 0xFF) as usize;
        let dci = ((event.control >> 16) & 0x1F) as usize;
        if slot_id == 0 || dci == 0 {
            return;
        }
        let Some(Some(slot)) = self.device_slots.get_mut(slot_id - 1) else {
            return;
        };
        // transfer_rings[] is indexed by DCI - 1 (same convention as the
        // doorbell path).
        let Some(Some(ring)) = slot.transfer_rings.get_mut(dci - 1) else {
            return;
        };
        let base = ring.phys_addr();
        let trb_ptr = event.parameter;
        if trb_ptr < base {
            return;
        }
        let idx = ((trb_ptr - base) / core::mem::size_of::<Trb>() as u64) as usize;
        // segment_size - 1 is the link TRB slot; completions never point there.
        if idx >= ring.segment_size - 1 {
            return;
        }
        ring.dequeue_index = (idx + 1) % (ring.segment_size - 1);
    }

    pub fn poll_events(&mut self) -> Vec<Trb> {
        let mut events = Vec::new();
        let rt_base = self.runtime_base();
        if let Some(ref mut event_ring) = self.event_ring {
            while let Some(trb) = event_ring.dequeue_event() {
                events.push(trb);
            }
            let intr0 = rt_base + 0x20;
            // Set EHB (bit 3) when advancing ERDP so the controller can post new events (xHCI §4.9.4).
            let erdp_val = (event_ring.dequeue_pointer() & !0xF) | 0x8;
            // Safety: 64-bit MMIO write split into two 32-bit volatile writes (low first).
            unsafe {
                core::ptr::write_volatile(
                    (self.mmio_base + intr0 + 0x18) as *mut u32,
                    erdp_val as u32,
                );
                core::ptr::write_volatile(
                    (self.mmio_base + intr0 + 0x18 + 4) as *mut u32,
                    (erdp_val >> 32) as u32,
                );
            }
        }
        // Credit each completion back to its transfer ring so the producer
        // never sees a phantom-full ring (see note_transfer_completion).
        for trb in &events {
            if trb.trb_type() == TrbType::TransferEvent {
                self.note_transfer_completion(trb);
            }
        }
        events
    }

    pub fn handle_port_status_change(&mut self, port_id: u8) -> Result<PortSpeed, XhciError> {
        if port_id == 0 || port_id > self.max_ports {
            return Err(XhciError::PortError);
        }
        let port_regs = self.read_port_registers(port_id);
        let speed = port_regs.port_speed();
        self.port_speeds[(port_id - 1) as usize] = speed;

        // Acknowledge change bits (write-1-to-clear), preserve PP, don't touch PED
        let portsc = self.read_port_sc(port_id);
        let clear_bits = PORTSC_CSC
            | PORTSC_PEC
            | PORTSC_WRC
            | PORTSC_OCC
            | PORTSC_PRC
            | PORTSC_PLC
            | PORTSC_CEC;
        self.write_port_sc(port_id, (portsc & PORTSC_PP) | clear_bits);

        Ok(speed)
    }

    pub fn reset_port(&self, port_id: u8) -> Result<(), XhciError> {
        if port_id == 0 || port_id > self.max_ports {
            return Err(XhciError::PortError);
        }
        let portsc = self.read_port_sc(port_id);

        // Port already enabled — firmware brought it to U0 (typical for the
        // USB3 boot stick on Athena: PED=1, PLS=0 at handoff). A reset here
        // would knock a working link back down; clear stale change bits and
        // proceed straight to Enable Slot. (PED is RW1C — masking to PP
        // below is what AVOIDS writing 1 to PED, which would disable it.)
        if portsc & PORTSC_PED != 0 {
            self.write_port_sc(port_id, (portsc & PORTSC_PP) | PORTSC_PRC | PORTSC_CSC);
            return Ok(());
        }

        self.write_port_sc(port_id, (portsc & PORTSC_PP) | PORTSC_PR);

        // Wall-clock wait: a USB2 root-port reset holds >= 50 ms (TDRSTR)
        // before PRC fires. The old fixed 100k-spin loop expired in
        // single-digit milliseconds on Athena's 4+ GHz cores (while passing
        // on QEMU, where reset completes instantly) — that was the
        // photographed "port N reset failed: Timeout" on every port. Also
        // accept PR-cleared+PED-set in case PRC was consumed via the event
        // ring's Port Status Change path first.
        let got = crate::hpet::spin_until_us(800_000, || {
            let sc = self.read_port_sc(port_id);
            sc & PORTSC_PRC != 0 || (sc & PORTSC_PR == 0 && sc & PORTSC_PED != 0)
        });
        if got {
            let sc = self.read_port_sc(port_id);
            self.write_port_sc(port_id, (sc & PORTSC_PP) | PORTSC_PRC);
            // USB 2.0 §7.1.7.5 TRSTRCY: the device gets >= 10 ms reset
            // recovery before it must accept transactions. QEMU tolerates
            // zero; real devices do not — Athena's port-2/port-4 devices
            // babbled/stalled their first GET_DESCRIPTOR when SET_ADDRESS
            // chased the reset within microseconds.
            //
            // ADR 0006 (boot-time gate): under QEMU the virtual root port needs
            // no TRSTRCY (it accepts transactions immediately), so this fixed
            // settle is pure boot-critical-path wall-clock paid per connected
            // device. Clamp to a token 500 us under QEMU; iron keeps the full
            // 10 ms. A present QEMU device still passes through this exact path
            // and arms unchanged — only the dead wait shrinks.
            let trstrcy_us = if cpuid_hv_vendor_is_qemu() {
                500
            } else {
                10_000
            };
            crate::hpet::spin_until_us(trstrcy_us, || false);
            // Reset "completed" with the port still disabled means the
            // device didn't survive reset — enumerating it anyway produced
            // the photographed garbage transfers. Fail loudly instead.
            let sc = self.read_port_sc(port_id);
            if sc & PORTSC_PED == 0 {
                crate::serial_println!(
                    "[xhci] port {} reset done but port not enabled (PORTSC={:#010x})",
                    port_id,
                    sc
                );
                return Err(XhciError::PortError);
            }
            return Ok(());
        }
        Err(XhciError::Timeout)
    }

    pub fn power_on_port(&self, port_id: u8) -> Result<(), XhciError> {
        if port_id == 0 || port_id > self.max_ports {
            return Err(XhciError::PortError);
        }
        let portsc = self.read_port_sc(port_id);
        if portsc & PORTSC_PP == 0 {
            self.write_port_sc(port_id, portsc | PORTSC_PP);
            // 20 ms wall-clock settle after applying power (was a fixed spin
            // count — meaningless across CPU speeds).
            //
            // ADR 0006 (boot-time gate): QEMU's virtual root port delivers good
            // power instantly (same fact the downstream-hub power-good settle is
            // already gated on, line ~5222), so this 20 ms is pure boot-path
            // wall-clock per port under QEMU. Token 1 ms under QEMU; iron keeps
            // the full 20 ms. Device detection/arming is unchanged on both.
            let pwr_settle_us = if cpuid_hv_vendor_is_qemu() {
                1_000
            } else {
                20_000
            };
            crate::hpet::spin_until_us(pwr_settle_us, || false);
        }
        Ok(())
    }

    fn read_port_sc(&self, port: u8) -> u32 {
        self.read_reg32(self.port_reg_base(port))
    }

    fn write_port_sc(&self, port: u8, value: u32) {
        self.write_reg32(self.port_reg_base(port), value)
    }

    fn read_port_registers(&self, port: u8) -> PortRegisters {
        let base = self.port_reg_base(port);
        PortRegisters {
            portsc: self.read_reg32(base),
            portpmsc: self.read_reg32(base + 0x04),
            portli: self.read_reg32(base + 0x08),
            porthlpmc: self.read_reg32(base + 0x0C),
        }
    }

    /// Rewind EP0 between control transfers — but ONLY under QEMU.
    ///
    /// Hardware-vs-emulator split (like `doorbell_target` and CSZ handling):
    ///   * **QEMU** xHCI emulation needs a clean EP0 ring before each control
    ///     transfer; without the Stop Endpoint + Set TR Dequeue rewind its 2nd
    ///     control transfer fails with a controller error (HCE) and HID never
    ///     arms (`armed 0`). So QEMU keeps the rewind.
    ///   * **Real AMD silicon (Athena)** HCE-halts on a Stop Endpoint issued
    ///     against EP0 right after the first device's descriptor read — the
    ///     controller dies before port 2's Enable Slot. So bare metal SKIPS the
    ///     rewind and relies on natural ring advancement (Setup/Data/Status
    ///     enqueue in sequence; the Link TRB handles wrap), the spec-correct
    ///     flow. `self.qemu_doorbell_offset` is the cached "is QEMU host" bit.
    fn rewind_ep0_if_qemu(&mut self, slot_id: u8) {
        if self.qemu_doorbell_offset {
            let _ = self.reset_ep0_transfer_ring(slot_id);
        }
    }

    /// Stop EP0, point the dequeue pointer at a cleared ring, and resume
    /// (endpoint ID 1). Called per-transfer under QEMU via
    /// [`Self::rewind_ep0_if_qemu`], and available for explicit error recovery.
    /// NOT used on bare metal's success path — the Stop Endpoint HCE-halts
    /// AMD's xHC (see `rewind_ep0_if_qemu`).
    pub fn reset_ep0_transfer_ring(&mut self, slot_id: u8) -> Result<(), XhciError> {
        let idx = (slot_id - 1) as usize;
        let phys = {
            let slot = self.device_slots[idx]
                .as_ref()
                .ok_or(XhciError::InvalidSlot)?;
            slot.transfer_rings
                .get(0)
                .and_then(|r| r.as_ref())
                .ok_or(XhciError::EndpointNotConfigured)?
                .phys_addr()
        };
        if let Some(ref mut ring) = self.device_slots[idx]
            .as_mut()
            .and_then(|s| s.transfer_rings[0].as_mut())
        {
            ring.clear_segment();
        }
        {
            let cmd_ring = self.command_ring.as_mut().ok_or(XhciError::NotReady)?;
            cmd_ring.stop_endpoint(slot_id, 1, false)?;
        }
        self.ring_doorbell_host();
        let event = self.wait_for_command(500_000)?;
        let code = TrbCompletionCode::from_u8(((event.status >> 24) & 0xFF) as u8);
        if code != TrbCompletionCode::Success {
            return Err(XhciError::CommandFailed(code));
        }
        {
            let cmd_ring = self.command_ring.as_mut().ok_or(XhciError::NotReady)?;
            cmd_ring.set_tr_dequeue_pointer(slot_id, 1, phys, false, 0)?;
        }
        self.ring_doorbell_host();
        let event = self.wait_for_command(500_000)?;
        let code = TrbCompletionCode::from_u8(((event.status >> 24) & 0xFF) as u8);
        if code != TrbCompletionCode::Success {
            return Err(XhciError::CommandFailed(code));
        }
        Ok(())
    }

    /// Align software EP0 producer index/cycle with the hardware output context (no commands).
    fn sync_ep_enqueue_from_hardware(&mut self, slot_id: u8, ep_index: u8) {
        let idx = (slot_id - 1) as usize;
        let ep_i = ep_index as usize;
        let ctx64 = self.context_size_64;
        let Some(slot) = self.device_slots[idx].as_mut() else {
            return;
        };
        let ep_ctx = slot.output_endpoint(ctx64, ep_i + 1);
        if let Some(ref mut ring) = slot.transfer_rings[ep_i] {
            ring.cycle_state = ep_ctx.dequeue_cycle_state();
            let base = ring.phys_addr();
            let dq = ep_ctx.tr_dequeue_pointer() & !0xF;
            if dq >= base {
                let trb_size = core::mem::size_of::<Trb>() as u64;
                let offset = ((dq - base) / trb_size) as usize;
                if offset < ring.segment_size.saturating_sub(1) {
                    ring.enqueue_index = offset;
                }
            }
        }
    }

    fn sync_ep0_enqueue_from_hardware(&mut self, slot_id: u8) {
        let idx = (slot_id - 1) as usize;
        let ctx64 = self.context_size_64;
        let Some(slot) = self.device_slots[idx].as_mut() else {
            return;
        };
        let ep0 = slot.output_endpoint(ctx64, 1);
        if let Some(ref mut ring) = slot.transfer_rings[0] {
            ring.cycle_state = ep0.dequeue_cycle_state();
            let base = ring.phys_addr();
            let dq = ep0.tr_dequeue_pointer() & !0xF;
            if dq >= base {
                let trb_size = core::mem::size_of::<Trb>() as u64;
                let offset = ((dq - base) / trb_size) as usize;
                if offset < ring.segment_size.saturating_sub(1) {
                    ring.enqueue_index = offset;
                }
            }
        }
    }

    /// Clear a halted EP0 pipe and reset its transfer ring (endpoint ID 1).
    pub fn reset_ep0_pipe(&mut self, slot_id: u8) -> Result<(), XhciError> {
        let idx = (slot_id - 1) as usize;
        let ep0 = self.device_slots[idx]
            .as_ref()
            .map(|s| s.output_endpoint(self.context_size_64, 1));
        if let Some(ep0) = ep0 {
            if ep0.ep_state() == EndpointState::Halted {
                let cmd_ring = self.command_ring.as_mut().ok_or(XhciError::NotReady)?;
                cmd_ring.reset_endpoint(slot_id, 1, false)?;
                self.ring_doorbell_host();
                let event = self.wait_for_command(500_000)?;
                let code = TrbCompletionCode::from_u8(((event.status >> 24) & 0xFF) as u8);
                if code != TrbCompletionCode::Success {
                    return Err(XhciError::CommandFailed(code));
                }
            }
        }
        let phys = self.device_slots[idx]
            .as_ref()
            .and_then(|s| s.transfer_rings[0].as_ref())
            .map(|r| r.phys_addr())
            .ok_or(XhciError::EndpointNotConfigured)?;
        self.command_ring
            .as_mut()
            .ok_or(XhciError::NotReady)?
            .reset_endpoint(slot_id, 1, true)?;
        self.ring_doorbell_host();
        let event = self.wait_for_command(500_000)?;
        let code = TrbCompletionCode::from_u8(((event.status >> 24) & 0xFF) as u8);
        if code != TrbCompletionCode::Success {
            return Err(XhciError::CommandFailed(code));
        }
        self.command_ring
            .as_mut()
            .ok_or(XhciError::NotReady)?
            .set_tr_dequeue_pointer(slot_id, 1, phys, false, 0)?;
        self.ring_doorbell_host();
        let event = self.wait_for_command(500_000)?;
        let code = TrbCompletionCode::from_u8(((event.status >> 24) & 0xFF) as u8);
        if code != TrbCompletionCode::Success {
            return Err(XhciError::CommandFailed(code));
        }
        if let Some(ring) = self.device_slots[idx]
            .as_mut()
            .and_then(|s| s.transfer_rings[0].as_mut())
        {
            ring.enqueue_index = 0;
            ring.dequeue_index = 0;
            ring.cycle_state = false;
        }
        Ok(())
    }

    /// After GET_DESCRIPTOR(Device), apply `bMaxPacketSize0` to EP0 contexts (USB 2.0 §9.6.1).
    pub fn apply_device_descriptor_ep0(&mut self, slot_id: u8, desc: &[u8; 18]) {
        let mps = desc[7] as u16;
        if mps == 0 {
            return;
        }
        let idx = (slot_id - 1) as usize;
        let ctx64 = self.context_size_64;
        if let Some(slot) = self.device_slots[idx].as_mut() {
            slot.max_packet_size_ep0 = mps;
            let ep0 = &mut slot.input_context().device.endpoints[0];
            ep0.set_max_packet_size(mps);
            unsafe {
                // Stride-aware: EP0 = output context entry 1 (64 B apart on
                // CSZ=1 controllers, 32 B on QEMU's CSZ=0).
                let stride = if ctx64 { 64usize } else { 32 };
                let out_ep0 = (slot.output_ctx.virt as *mut u8).add(stride) as *mut EndpointContext;
                (*out_ep0).set_max_packet_size(mps);
            }
        }
    }

    pub fn submit_control_transfer(
        &mut self,
        slot_id: u8,
        setup: &[u8; 8],
        data: Option<(&[u8], u64)>,
        direction_in: bool,
    ) -> Result<(), XhciError> {
        let idx = (slot_id - 1) as usize;
        let slot = self.device_slots[idx]
            .as_mut()
            .ok_or(XhciError::InvalidSlot)?;

        // A control transfer is THREE independent Transfer Descriptors
        // (xHCI §4.11.2.2): Setup TD, Data TD, Status TD. The Chain bit links
        // TRBs WITHIN one TD — it must NOT span stages. Chaining Setup→Data
        // (the previous code) makes a malformed mixed-stage TD that AMD's xHC
        // rejects by halting with HCE (Athena photo #5: USBSTS HCH+HCE,
        // HSE=false); QEMU tolerates it. Each stage here is a single-TRB TD,
        // so every Chain bit is 0.
        let mut setup_trb = Trb::new();
        setup_trb.set_trb_type(TrbType::SetupStage);
        setup_trb.parameter = u64::from_le_bytes(*setup);
        setup_trb.status = 8;
        setup_trb.set_immediate_data(true);
        let trt = if data.is_some() {
            if direction_in {
                3
            } else {
                2
            }
        } else {
            0
        };
        setup_trb.control |= trt << 16;
        setup_trb.set_ioc(false);
        slot.enqueue_transfer(0, setup_trb)?;

        if let Some((buf, buf_phys)) = data {
            let mut data_trb = Trb::new();
            data_trb.set_trb_type(TrbType::DataStage);
            data_trb.parameter = buf_phys;
            data_trb.status = buf.len() as u32;
            if direction_in {
                data_trb.control |= 1 << 16;
            }
            data_trb.set_ioc(false);
            slot.enqueue_transfer(0, data_trb)?;
        }

        let mut status_trb = Trb::new();
        status_trb.set_trb_type(TrbType::StatusStage);
        // DIR bit: 1 = host OUT. After data-IN status is OUT; after data-OUT status is IN.
        if data.is_some() && direction_in {
            status_trb.control |= 1 << 16;
        } else if data.is_none() {
            status_trb.control |= 1 << 16;
        }
        status_trb.set_ioc(true);
        slot.enqueue_transfer(0, status_trb)?;

        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        self.ring_doorbell_device(slot_id, self.doorbell_target(0), 0);
        Ok(())
    }

    pub fn submit_bulk_transfer(
        &mut self,
        slot_id: u8,
        ep_index: u8,
        data_ptr: u64,
        length: u32,
    ) -> Result<(), XhciError> {
        let idx = (slot_id - 1) as usize;
        let slot = self.device_slots[idx]
            .as_mut()
            .ok_or(XhciError::InvalidSlot)?;

        let mut trb = Trb::new();
        trb.set_trb_type(TrbType::Normal);
        trb.parameter = data_ptr;
        trb.status = length;
        trb.set_ioc(true);
        slot.enqueue_transfer(ep_index, trb)?;

        self.ring_doorbell_device(slot_id, self.doorbell_target(ep_index), 0);
        Ok(())
    }

    pub fn submit_interrupt_transfer(
        &mut self,
        slot_id: u8,
        ep_index: u8,
        data_ptr: u64,
        length: u32,
    ) -> Result<(), XhciError> {
        self.submit_interrupt_in(slot_id, ep_index, data_ptr, length)
    }

    pub fn submit_isoch_transfer(
        &mut self,
        slot_id: u8,
        ep_index: u8,
        data_ptr: u64,
        length: u32,
        frame_id: u16,
    ) -> Result<(), XhciError> {
        let idx = (slot_id - 1) as usize;
        let slot = self.device_slots[idx]
            .as_mut()
            .ok_or(XhciError::InvalidSlot)?;

        let mut trb = Trb::new();
        trb.set_trb_type(TrbType::Isoch);
        trb.parameter = data_ptr;
        trb.status = length;
        if frame_id == 0 {
            // SIA (Start Isoch ASAP, control bit 31): let the xHC schedule this
            // TD in the next available service interval. Simplest correct mode
            // for stream start; frame_id is ignored when SIA is set (xHCI §6.4.1.3).
            trb.control |= 1 << 31;
        } else {
            trb.control |= ((frame_id as u32) & 0x7FF) << 20;
        }
        trb.set_ioc(true);
        slot.enqueue_transfer(ep_index, trb)?;

        self.ring_doorbell_device(slot_id, self.doorbell_target(ep_index), 0);
        Ok(())
    }

    pub fn process_event(&mut self, trb: &Trb) -> Result<(), XhciError> {
        match trb.trb_type() {
            TrbType::TransferEvent => self.handle_transfer_event(trb),
            TrbType::CommandCompletionEvent => self.handle_command_completion(trb),
            TrbType::PortStatusChangeEvent => {
                let port_id = ((trb.parameter >> 24) & 0xFF) as u8;
                self.handle_port_status_change(port_id)?;
                Ok(())
            }
            TrbType::HostControllerEvent => {
                let code = TrbCompletionCode::from_u8(((trb.status >> 24) & 0xFF) as u8);
                if code != TrbCompletionCode::Success {
                    return Err(XhciError::HostError);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn handle_transfer_event(&self, trb: &Trb) -> Result<(), XhciError> {
        let code = TrbCompletionCode::from_u8(((trb.status >> 24) & 0xFF) as u8);
        match code {
            TrbCompletionCode::Success | TrbCompletionCode::ShortPacket => Ok(()),
            _ => Err(XhciError::TransferFailed(code)),
        }
    }

    fn handle_command_completion(&self, trb: &Trb) -> Result<(), XhciError> {
        let code = TrbCompletionCode::from_u8(((trb.status >> 24) & 0xFF) as u8);
        if code != TrbCompletionCode::Success {
            return Err(XhciError::CommandFailed(code));
        }
        Ok(())
    }

    pub fn device_connected(&self) -> bool {
        self.device_slots.iter().any(|s| s.is_some())
    }

    pub fn active_slot_count(&self) -> usize {
        self.device_slots.iter().filter(|s| s.is_some()).count()
    }

    // ─── USB Hub class requests (USB 2.0 §11.24) ─────────────────────────────

    /// Generic control-IN, returning the bytes actually transferred.
    fn control_in_vec(
        &mut self,
        slot_id: u8,
        bm_request_type: u8,
        b_request: u8,
        w_value: u16,
        w_index: u16,
        length: u16,
    ) -> Result<Vec<u8>, XhciError> {
        let len = length as usize;
        if len == 0 || len > 4096 {
            return Err(XhciError::InvalidState);
        }
        let page = alloc_dma_page()?;
        let buf = unsafe { core::slice::from_raw_parts_mut(page.virt as *mut u8, len) };
        let setup: [u8; 8] = [
            bm_request_type,
            b_request,
            (w_value & 0xFF) as u8,
            (w_value >> 8) as u8,
            (w_index & 0xFF) as u8,
            (w_index >> 8) as u8,
            (length & 0xFF) as u8,
            (length >> 8) as u8,
        ];
        self.submit_control_transfer(slot_id, &setup, Some((buf, page.phys)), true)?;
        let event = self.wait_for_transfer(500_000)?;
        let residual = (event.status & 0x00FF_FFFF) as usize;
        let got = len.saturating_sub(residual);
        let out = buf[..got].to_vec();
        self.rewind_ep0_if_qemu(slot_id);
        // DMA complete + copied out — free the transient page (see get_descriptor).
        page.free();
        Ok(out)
    }

    /// Generic control-OUT with no data stage (SET_FEATURE / CLEAR_FEATURE).
    fn control_out_nodata(
        &mut self,
        slot_id: u8,
        bm_request_type: u8,
        b_request: u8,
        w_value: u16,
        w_index: u16,
    ) -> Result<(), XhciError> {
        let setup: [u8; 8] = [
            bm_request_type,
            b_request,
            (w_value & 0xFF) as u8,
            (w_value >> 8) as u8,
            (w_index & 0xFF) as u8,
            (w_index >> 8) as u8,
            0x00,
            0x00,
        ];
        self.submit_control_transfer(slot_id, &setup, None, false)?;
        self.wait_for_transfer(500_000)?;
        self.rewind_ep0_if_qemu(slot_id);
        Ok(())
    }

    /// GET hub descriptor: class type 0x29 for USB2 hubs, 0x2A for USB3
    /// (SuperSpeed) hubs. A USB3 hub MUST stall a 0x29 request — exactly the
    /// photographed Athena failure: the VIA USB3 hub (2109:0822) stalled
    /// hub_get_descriptor, so everything behind it (the boot stick) never
    /// enumerated.
    fn hub_get_descriptor(&mut self, slot_id: u8, usb3: bool) -> Result<Vec<u8>, XhciError> {
        let dtype: u16 = if usb3 { 0x2A } else { 0x29 };
        self.control_in_vec(slot_id, 0xA0, 0x06, dtype << 8, 0, 0x40)
    }

    /// USB3 SET_HUB_DEPTH (class request 12): tells a SuperSpeed hub how many
    /// hubs sit between it and the root (0 for a root-port hub) so it can
    /// strip the right route-string nibbles. Mandatory before port requests.
    fn hub_set_depth(&mut self, slot_id: u8, depth: u16) -> Result<(), XhciError> {
        self.control_out_nodata(slot_id, 0x20, 12, depth, 0)
    }

    /// SetPortFeature on a hub downstream port (PORT_POWER=8, PORT_RESET=4).
    fn hub_set_port_feature(
        &mut self,
        slot_id: u8,
        port: u8,
        feature: u16,
    ) -> Result<(), XhciError> {
        self.control_out_nodata(slot_id, 0x23, 0x03, feature, port as u16)
    }

    /// ClearPortFeature on a hub downstream port (e.g. C_PORT_RESET=20).
    fn hub_clear_port_feature(
        &mut self,
        slot_id: u8,
        port: u8,
        feature: u16,
    ) -> Result<(), XhciError> {
        self.control_out_nodata(slot_id, 0x23, 0x01, feature, port as u16)
    }

    /// GetPortStatus → (wPortStatus, wPortChange).
    fn hub_get_port_status(&mut self, slot_id: u8, port: u8) -> Result<(u16, u16), XhciError> {
        let data = self.control_in_vec(slot_id, 0xA3, 0x00, 0, port as u16, 4)?;
        if data.len() < 4 {
            return Err(XhciError::InvalidState);
        }
        Ok((
            u16::from_le_bytes([data[0], data[1]]),
            u16::from_le_bytes([data[2], data[3]]),
        ))
    }

    /// Mark an addressed device slot as a hub in the xHC (Configure Endpoint with
    /// A0 set updates the slot context Hub/NumberOfPorts/TTT/MTT fields).
    fn configure_hub_slot(
        &mut self,
        slot_id: u8,
        num_ports: u8,
        ttt: u8,
        multi_tt: bool,
    ) -> Result<(), XhciError> {
        let idx = (slot_id - 1) as usize;
        let input_phys = {
            let slot = self.device_slots[idx]
                .as_mut()
                .ok_or(XhciError::InvalidSlot)?;
            slot.prepare_configure_hub(num_ports, ttt, multi_tt);
            slot.input_ctx.phys
        };
        self.configure_endpoint_with_ptr(slot_id, input_phys)
    }

    /// Enumerate every populated downstream port of an addressed hub: power the
    /// port, reset it, address the attached device behind the hub (route string +
    /// TT), then either recurse (nested hub) or bring up its HID endpoint.
    ///
    /// `route` is the parent hub's route string; `depth` is the hub's tier
    /// (1 = hub on a root port). `root_port` is the top-level root-hub port the
    /// whole chain hangs off. MasterChecklist Phase 2.1: USB hub support.
    fn enumerate_hub(
        &mut self,
        hub_slot: u8,
        route: u32,
        depth: u8,
        root_port: u8,
        hub_protocol: u8,
    ) -> Result<(), XhciError> {
        // USB allows 5 hub tiers below the root; cap to avoid runaway recursion.
        if depth > 5 {
            crate::serial_println!("[usb-hub] tier cap reached at depth {} — stopping", depth);
            return Ok(());
        }

        // bDeviceProtocol from the DEVICE descriptor: 0 = FS hub, 1 = HS
        // single-TT, 2 = HS multi-TT, 3 = SuperSpeed hub. USB3 hubs use a
        // different hub-descriptor type and need SET_HUB_DEPTH.
        let usb3 = hub_protocol == 3;
        let desc = self.hub_get_descriptor(hub_slot, usb3)?;
        if desc.len() < 5 {
            return Err(XhciError::InvalidState);
        }
        let num_ports = desc[2];
        let w_hub_char = u16::from_le_bytes([desc[3], desc[4]]);
        let ttt = ((w_hub_char >> 5) & 0x3) as u8;
        let multi_tt = hub_protocol == 2;
        crate::serial_println!(
            "[usb-hub] slot {} is a hub: {} ports, ttt={}, mtt={} (tier {})",
            hub_slot,
            num_ports,
            ttt,
            multi_tt,
            depth
        );

        // Stale command-completion events from the caller's command sequence
        // would otherwise be misread as our control-transfer completions.
        self.deferred_events.clear();

        // Hub must be configured before SetPortFeature works — an
        // unconfigured hub STALLs every class request, which is the
        // photographed Athena failure mode (VIA 2109:2822: descriptor +
        // ConfigureEndpoint OK, then everything stalled, so the devices
        // behind it — keyboard/mouse/boot stick — never enumerated). Check
        // the result and run one EP0-recovery retry instead of ignoring it.
        if let Err(e) = self.set_configuration(hub_slot, 1) {
            crate::serial_println!(
                "[usb-hub] SET_CONFIGURATION(slot {}) failed: {:?} — EP0 recovery + retry",
                hub_slot,
                e
            );
            if let Err(re) = self.recover_ep0(hub_slot) {
                crate::serial_println!("[usb-hub] EP0 recovery failed: {:?}", re);
            }
            crate::hpet::spin_until_us(10_000, || false);
            match self.set_configuration(hub_slot, 1) {
                Ok(()) => crate::serial_println!("[usb-hub] SET_CONFIGURATION retry OK"),
                Err(e2) => crate::serial_println!(
                    "[usb-hub] SET_CONFIGURATION retry failed: {:?} — port requests will likely stall",
                    e2
                ),
            }
        }
        // SET_HUB_DEPTH BEFORE the xHC hub-marking (configure_hub_slot). It is a
        // DEVICE class request — it goes to the hub's own EP0, not the host
        // controller — so it does NOT need the slot marked as a hub first, and a
        // SuperSpeed hub answers it independent of the HCD's slot-context state
        // (Linux sends it from the hub driver over EP0). Boot 024544 diagnosis
        // (confirmed against the live Athena hub on Linux, which enumerates clean):
        // SET_CONFIGURATION — the same no-data EP0 control path — SUCCEEDS, then
        // configure_hub_slot's ConfigureEndpoint runs, then SET_HUB_DEPTH gets NO
        // TransferEvent (EP0 left Running). The hub-marking ConfigureEndpoint is
        // what perturbs EP0; doing SET_HUB_DEPTH first sidesteps it. Mandatory for
        // SuperSpeed hubs before any port request (so the hub strips the right
        // route-string nibbles).
        if usb3 {
            let mut depth_ok = self.hub_set_depth(hub_slot, (depth - 1) as u16).is_ok();
            if !depth_ok {
                // One EP0-recovery retry (same class as the keyboard recover-first
                // fix) before abandoning the hub and the boot stick behind it.
                crate::serial_println!(
                    "[usb-hub] SET_HUB_DEPTH timed out (USB3 hub slot {}) — EP0 recovery + retry",
                    hub_slot
                );
                let _ = self.recover_ep0(hub_slot);
                crate::hpet::spin_until_us(10_000, || false);
                depth_ok = self.hub_set_depth(hub_slot, (depth - 1) as u16).is_ok();
            }
            if depth_ok {
                crate::serial_println!(
                    "[usb-hub] SET_HUB_DEPTH({}) OK (USB3 hub slot {})",
                    depth - 1,
                    hub_slot
                );
            } else {
                crate::serial_println!(
                    "[usb-hub] SET_HUB_DEPTH still failing after retry — recovering EP0, skipping port scan"
                );
                let _ = self.recover_ep0(hub_slot);
                return Ok(());
            }
        }
        // Now mark the slot as a hub in the xHC (Hub/NumberOfPorts/TTT/MTT) so it
        // routes to the downstream ports — AFTER SET_HUB_DEPTH so the
        // ConfigureEndpoint can't perturb that EP0 control transfer.
        if let Err(e) = self.configure_hub_slot(hub_slot, num_ports, ttt, multi_tt) {
            crate::serial_println!(
                "[usb-hub] configure_hub_slot(slot {}) failed: {:?}",
                hub_slot,
                e
            );
        }

        // bPwrOn2PwrGood (hub descriptor byte 5, 2ms units) is how long the hub
        // takes to deliver good power after PORT_POWER. A fixed 20ms was too
        // short for hubs that report more — the port shows no CONNECTION yet, so
        // a real device (the install stick) is declared absent and never
        // enumerates. Honor the descriptor, floored at the 20ms we had.
        //
        // ADR 0006 (boot-time gate): under QEMU the virtual hub delivers power
        // and connection state instantly, so the per-port power-good settle is
        // pure boot-critical-path wall-clock (8 ports × 20 ms ≈ 160 ms of the
        // ~860 ms xhci_smoke). QEMU-gate it to a token 1 ms; iron timing is
        // byte-identical (real hubs keep the descriptor-honored settle), and
        // every device still enumerates + arms exactly as before on both.
        let is_qemu_hub = cpuid_hv_vendor_is_qemu();
        let pwr_good_us = if is_qemu_hub {
            1_000
        } else {
            ((desc.get(5).copied().unwrap_or(10) as u64) * 2_000).max(20_000)
        };

        for d in 1..=num_ports {
            let _ = self.hub_set_port_feature(hub_slot, d, 8); // PORT_POWER
            let _ = crate::hpet::spin_until_us(pwr_good_us, || false); // power-on settle

            let (mut status, _change) = match self.hub_get_port_status(hub_slot, d) {
                Ok(s) => s,
                Err(e) => {
                    // Was a silent `continue` — on Athena that hid WHY the
                    // hub's children never appeared. Say it, then recover
                    // EP0 so the stall doesn't poison the next port too.
                    crate::serial_println!(
                        "[usb-hub] GetPortStatus(slot {} port {}) failed: {:?}",
                        hub_slot,
                        d,
                        e
                    );
                    let _ = self.recover_ep0(hub_slot);
                    continue;
                }
            };
            // A device can lag power-up: poll CONNECTION for up to ~150ms before
            // declaring the port empty (the install-stick-never-enumerates case).
            // ADR 0006: QEMU reports connection state instantly, so the 150 ms
            // empty-port poll is pure boot-path wall-clock there — clamp it to
            // 2 ms under QEMU. Iron keeps the full 150 ms (a slow real stick must
            // still be found); a present QEMU device passes the first check and
            // never enters this branch, so arming is unchanged on both.
            let conn_poll_us: u64 = if is_qemu_hub { 2_000 } else { 150_000 };
            if status & 0x0001 == 0 {
                let _ = crate::hpet::spin_until_us(conn_poll_us, || {
                    match self.hub_get_port_status(hub_slot, d) {
                        Ok((s, _)) => {
                            status = s;
                            s & 0x0001 != 0
                        }
                        Err(_) => false,
                    }
                });
            }
            if status & 0x0001 == 0 {
                continue; // no device connected (PORT_CONNECTION clear)
            }
            crate::serial_println!(
                "[usb-hub] slot {} port {} power-good after {}us",
                hub_slot,
                d,
                pwr_good_us
            );
            crate::serial_println!(
                "[usb-hub] slot {} port {} connected (status={:#06x})",
                hub_slot,
                d,
                status
            );

            // Reset the downstream port and wait for it to enable.
            let _ = self.hub_set_port_feature(hub_slot, d, 4); // PORT_RESET
            let mut final_status = 0u16;
            let _ = crate::hpet::spin_until_us(200_000, || {
                match self.hub_get_port_status(hub_slot, d) {
                    Ok((s, c)) => {
                        final_status = s;
                        // Reset done: RESET cleared & ENABLE set, or C_PORT_RESET latched.
                        ((s & (1 << 4)) == 0 && (s & (1 << 1)) != 0) || (c & (1 << 4)) != 0
                    }
                    Err(_) => false,
                }
            });
            if let Ok((s, _)) = self.hub_get_port_status(hub_slot, d) {
                final_status = s;
            }
            let _ = self.hub_clear_port_feature(hub_slot, d, 20); // C_PORT_RESET
            let _ = crate::hpet::spin_until_us(10_000, || false); // reset recovery

            if final_status & (1 << 1) == 0 {
                crate::serial_println!(
                    "[usb-hub] slot {} port {} not enabled after reset (status={:#06x})",
                    hub_slot,
                    d,
                    final_status
                );
                continue;
            }

            // Port-status speed bits are USB2-hub-only (bit 9 = LOW_SPEED,
            // bit 10 = HIGH_SPEED). On a USB3 hub bit 9 is PORT_POWER and
            // every child is SuperSpeed — decoding USB2 bits there would
            // misread a powered SS port as a LowSpeed device.
            let child_speed = if usb3 {
                PortSpeed::SuperSpeed
            } else if final_status & (1 << 9) != 0 {
                PortSpeed::LowSpeed
            } else if final_status & (1 << 10) != 0 {
                PortSpeed::HighSpeed
            } else {
                PortSpeed::FullSpeed
            };
            // Route string: append this downstream port at the hub's tier nibble.
            let child_route = route | (((d.min(15)) as u32) << (4 * (depth - 1)));

            let child_slot = match self.enable_slot() {
                Ok(s) => s,
                Err(e) => {
                    crate::serial_println!("[usb-hub] enable_slot failed: {:?}", e);
                    continue;
                }
            };
            let parent = ParentHub {
                slot_id: hub_slot,
                port: d,
                tt_think_time: ttt,
                multi_tt,
            };
            if let Err(e) = self.address_device_routed(
                child_slot,
                root_port,
                child_route,
                child_speed,
                Some(parent),
                false,
            ) {
                crate::serial_println!(
                    "[usb-hub] address_device(slot {}, route {:#x}) failed: {:?}",
                    child_slot,
                    child_route,
                    e
                );
                let _ = self.disable_slot(child_slot);
                continue;
            }

            if matches!(child_speed, PortSpeed::FullSpeed | PortSpeed::LowSpeed) {
                self.ensure_fs_ep0_mps(child_slot);
            }
            let dev = match self.get_device_descriptor_with_recovery(child_slot) {
                Ok(d) => d,
                Err(e) => {
                    crate::serial_println!(
                        "[usb-hub] get_device_descriptor(slot {}) failed: {:?}",
                        child_slot,
                        e
                    );
                    continue;
                }
            };
            let vendor = u16::from_le_bytes([dev[8], dev[9]]);
            let product = u16::from_le_bytes([dev[10], dev[11]]);
            let dclass = dev[4];
            let dprotocol = dev[6];
            crate::serial_println!(
                "[usb-hub] downstream slot {} class={} vid={:04x} pid={:04x} speed={:?}",
                child_slot,
                dclass,
                vendor,
                product,
                child_speed
            );

            if dclass == 0x09 {
                if let Err(e) =
                    self.enumerate_hub(child_slot, child_route, depth + 1, root_port, dprotocol)
                {
                    crate::serial_println!("[usb-hub] nested hub enum failed: {:?}", e);
                }
            } else {
                // Full classification, same as root ports: a hub child can be a
                // USB stick (MSC) or audio device, not just a keyboard. The
                // previous HID-only assumption made a boot stick in a front
                // (hub-routed) port permanently invisible to storage.
                self.classify_and_bring_up(child_slot, vendor, product, "usb-hub");
            }
        }
        Ok(())
    }

    /// Classify an addressed non-hub device by its configuration descriptor
    /// (USB Audio → MSC → HID, in that order) and run the matching bring-up.
    /// Shared by root-port enumeration and hub-child enumeration so every
    /// attachment point supports every device class. `tag` prefixes the log
    /// lines (`xhci` for root ports, `usb-hub` for hub children).
    pub fn classify_and_bring_up(
        &mut self,
        slot_id: u8,
        vendor: u16,
        product: u16,
        tag: &str,
    ) -> BroughtUp {
        let config = self.get_descriptor(slot_id, 2, 0, 255).ok();
        let uac = config
            .as_ref()
            .map(|c| crate::usb_audio::parse_config(c))
            .filter(|u| !u.streams.is_empty());
        let is_msc = config
            .as_ref()
            .and_then(|c| crate::xhci_desc::find_msc_bulk(c))
            .is_some();
        if let Some(u) = uac {
            // MasterChecklist 2.6: real-device UAC detection + isochronous
            // endpoint bring-up.
            crate::serial_println!(
                "[usb-audio] slot {} {:04x}:{:04x}: UAC{} detected — {} stream(s){}",
                slot_id,
                vendor,
                product,
                if u.uac_version >= 0x0200 { 2 } else { 1 },
                u.streams.len(),
                if u.playback_stream().is_some() {
                    ", playback"
                } else {
                    ""
                },
            );
            match self.bring_up_audio(slot_id) {
                Ok(()) => {
                    crate::serial_println!(
                        "[usb-audio] bring-up OK: isoch OUT endpoint configured + stream armed (slot {})",
                        slot_id
                    );
                    BroughtUp::Audio
                }
                Err(e) => {
                    crate::serial_println!("[usb-audio] bring-up failed: {:?}", e);
                    BroughtUp::Failed
                }
            }
        } else if is_msc {
            match self.bring_up_msc(slot_id, vendor, product) {
                Ok(()) => {
                    crate::serial_println!(
                        "[{}] USB Mass Storage: bulk endpoints configured + SET_CONFIGURATION OK (slot {})",
                        tag,
                        slot_id
                    );
                    BroughtUp::Msc
                }
                Err(e) => {
                    crate::serial_println!("[{}] MSC bring-up failed: {:?}", tag, e);
                    BroughtUp::Failed
                }
            }
        } else {
            match self.bring_up_hid_keyboard(slot_id, vendor, product) {
                Ok(()) => {
                    crate::serial_println!(
                        "[{}] HID boot device: SET_CONFIGURATION + interrupt IN OK (slot {})",
                        tag,
                        slot_id
                    );
                    BroughtUp::Hid
                }
                Err(e) => {
                    crate::serial_println!("[{}] HID bring-up failed: {:?}", tag, e);
                    BroughtUp::Failed
                }
            }
        }
    }
}

/// What [`XhciController::classify_and_bring_up`] managed to configure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BroughtUp {
    Audio,
    Msc,
    Hid,
    Failed,
}

/// FAIL-able pure model of the HID interrupt-IN transfer-ring recycle invariant
/// (BUG 1). Mirrors the exact producer/consumer math of `TransferRing`
/// (`enqueue`/`advance_enqueue`/`is_full`) + `note_transfer_completion`'s credit
/// + the `service_hid_reports` top-up, so the phantom-full-after-N-events class
/// (CLAUDE.md pitfall #8 — the SILENT wedge after ~16 reports) is caught by a
/// FAIL-able boot check on EVERY boot, not only behind a real (or QEMU) xHC.
/// Drives more completions than a full ring lap, asserting the endpoint never
/// drains to a permanent stall and `is_full` never trips. Err(reason) on any
/// invariant violation.
fn hid_ring_recycle_model_check() -> Result<(), &'static str> {
    const SEG: usize = RING_SEGMENT_SIZE; // 256
    const USABLE: usize = SEG - 1; // 255 (index SEG-1 is the Link TRB)
    const DEPTH: u64 = HID_TD_RING; // 16 outstanding

    // Software ring producer (mirror of TransferRing::{enqueue,advance,is_full}).
    let mut enqueue = 0usize;
    let mut dequeue = 0usize;
    let mut cycle = true;
    let is_full = |enq: usize, deq: usize| -> bool { (enq + 1) % USABLE == deq };
    let enq = |enqueue: &mut usize, cycle: &mut bool, deq: usize| -> Option<usize> {
        if is_full(*enqueue, deq) {
            return None;
        }
        let idx = *enqueue;
        *enqueue += 1;
        if *enqueue >= SEG - 1 {
            *cycle = !*cycle;
            *enqueue = 0;
        }
        Some(idx)
    };

    // Initial arm: DEPTH TDs, recording their ring positions in FIFO order.
    use alloc::collections::VecDeque;
    let mut outstanding: VecDeque<usize> = VecDeque::new();
    for _ in 0..DEPTH {
        match enq(&mut enqueue, &mut cycle, dequeue) {
            Some(idx) => outstanding.push_back(idx),
            None => return Err("init: could not arm initial HID_TD_RING TDs"),
        }
    }
    if outstanding.len() as u64 != DEPTH {
        return Err("init: outstanding != HID_TD_RING after arm");
    }

    // Drive 3 full ring laps of completions. Each step: the controller completes
    // the oldest TD (credit dequeue via note_transfer_completion math), the drain
    // re-arms one, and the top-up restores any shortfall to DEPTH.
    let steps = USABLE * 3;
    for n in 0..steps {
        let comp = outstanding
            .pop_front()
            .ok_or("steady: endpoint drained to EMPTY (would wedge — no event to re-arm from)")?;
        // note_transfer_completion: dequeue = (idx + 1) % USABLE.
        if comp >= USABLE {
            return Err("credit: completion pointed at the Link TRB slot");
        }
        dequeue = (comp + 1) % USABLE;
        // Per-completion re-arm.
        match enq(&mut enqueue, &mut cycle, dequeue) {
            Some(idx) => outstanding.push_back(idx),
            None => {
                return Err("steady: re-arm hit phantom-full ring (credit not advancing dequeue)")
            }
        }
        // Top-up to DEPTH (the BUG 1 indefinite-flow guard).
        while (outstanding.len() as u64) < DEPTH {
            match enq(&mut enqueue, &mut cycle, dequeue) {
                Some(idx) => outstanding.push_back(idx),
                None => break,
            }
        }
        if (outstanding.len() as u64) != DEPTH {
            return Err("top-up: failed to restore HID_TD_RING outstanding depth");
        }
        let _ = n;
    }
    // After 3 laps the endpoint is still at full depth — flows indefinitely.
    if (outstanding.len() as u64) != DEPTH {
        return Err("post-laps: outstanding depth not maintained (would wedge over time)");
    }
    Ok(())
}

pub fn run_boot_smoketest() {
    crate::serial_println!("[xhci] Running boot smoketest...");

    // HID transfer-ring recycle invariant — runs on every boot (the live xHC
    // only exists on iron/QEMU, but this pure model proves the recycle + top-up
    // math everywhere, FAIL-ably). This is the BUG 1 guard.
    match hid_ring_recycle_model_check() {
        Ok(()) => crate::serial_println!(
            "[xhci] HID-ring recycle model: {} TDs, 3 laps, credit+top-up held -> PASS",
            HID_TD_RING
        ),
        Err(reason) => crate::serial_println!(
            "[xhci] HID-ring recycle model -> FAIL: {} (HID would wedge after N events)",
            reason
        ),
    }

    let mut total_armed = 0usize;
    {
        let mut ctrl = XHCI_CONTROLLER.lock();
        if !ctrl.initialized {
            crate::serial_println!("[xhci] Controller not initialized (maybe no hardware found)");
            return;
        }
        total_armed += enumerate_controller(&mut ctrl);
    }
    // Same full bring-up for every secondary controller — on Athena that is
    // c4:00.4 + c6:00.3 + c6:00.4, i.e. all the physical port groups that
    // were previously dead (keyboard in a rear port, the boot USB stick).
    {
        let mut secondaries = XHCI_SECONDARY.lock();
        for (i, ctrl) in secondaries.iter_mut().enumerate() {
            crate::serial_println!("[xhci] ── enumerating secondary controller #{} ──", i + 1);
            total_armed += enumerate_controller(ctrl);
        }
    }
    HID_ARMED_TOTAL.store(total_armed, core::sync::atomic::Ordering::Relaxed);
    if total_armed > 0 {
        spawn_hid_input_thread();
    }

    crate::serial_println!("[xhci] smoketest passed");
}

/// One controller's full bring-up: register check, port scan, per-port
/// enumeration (device / hub / MSC / HID classification), the deferred-hub
/// pass, and HID interrupt-IN arming. Returns the number of HID endpoints
/// armed on THIS controller (the caller sums across controllers to decide
/// whether to spawn the input-servicing thread).
fn enumerate_controller(ctrl: &mut XhciController) -> usize {
    crate::serial_println!(
        "[xhci] Controller live: {} slots, {} ports, context_64={}",
        ctrl.max_slots,
        ctrl.max_ports,
        ctrl.context_size_64
    );

    // Basic register check
    let usb_sts = ctrl.read_op_reg(0x04);
    crate::serial_println!(
        "[xhci] USBSTS: {:#010x} (HCH={}, CNR={})",
        usb_sts,
        usb_sts & 1 != 0,
        (usb_sts >> 11) & 1 != 0
    );

    // Port scan
    let max_ports = ctrl.max_ports;
    let mut connected = 0;
    for i in 1..=max_ports {
        let sc = ctrl.read_port_sc(i);
        if sc & PORTSC_CCS != 0 {
            connected += 1;
            let speed = (sc & PORTSC_SPEED_MASK) >> 10;
            crate::serial_println!("[xhci] Port {}: Device connected (Speed ID: {})", i, speed);
        }
    }
    crate::serial_println!(
        "[xhci] Scan complete: {} devices found on root hub",
        connected
    );

    // Hubs are enumerated AFTER every direct device, because hub enumeration
    // is the riskiest path (extra control transfers + a slot-context command)
    // and on real hardware a buggy hub can halt the whole controller (Athena:
    // a VIA hub on port 1 HCE-halted the xHC before the keyboard/mouse ports
    // were ever reached, so they never armed — "mouse lights up but the cursor
    // never moves"). Deferring lets the direct HID/MSC devices come up while
    // the controller is healthy. Each entry: (hub_slot, root_port, hub_protocol).
    let mut deferred_hubs: alloc::vec::Vec<(u8, u8, u8)> = alloc::vec::Vec::new();
    let mut direct_hid_brought_up = 0u32;
    let mut direct_msc_brought_up = 0u32;

    if connected > 0 {
        for port in 1..=max_ports {
            let sc = ctrl.read_port_sc(port);
            if sc & PORTSC_CCS == 0 {
                continue;
            }
            crate::serial_println!("[xhci] Bringing up port {}...", port);
            let sc0 = ctrl.read_port_sc(port);
            crate::serial_println!(
                "[xhci] port {} PORTSC={:#010x} PED={} PP={} PLS={}",
                port,
                sc0,
                sc0 & PORTSC_PED != 0,
                sc0 & PORTSC_PP != 0,
                (sc0 & PORTSC_PLS_MASK) >> 5
            );
            let _ = ctrl.power_on_port(port);
            if let Err(e) = ctrl.reset_port(port) {
                crate::serial_println!("[xhci] port {} reset failed: {:?}", port, e);
                continue;
            }
            let sc = ctrl.read_port_sc(port);
            let speed_bits = (sc & PORTSC_SPEED_MASK) >> 10;
            let speed = match speed_bits {
                1 => PortSpeed::FullSpeed,
                2 => PortSpeed::LowSpeed,
                3 => PortSpeed::HighSpeed,
                4 => PortSpeed::SuperSpeed,
                5 => PortSpeed::SuperSpeedPlus,
                _ => PortSpeed::Undefined,
            };
            match ctrl.enable_slot() {
                Ok(slot_id) => {
                    crate::serial_println!("[xhci] Enable Slot -> slot {}", slot_id);
                    // BSR=false: xHC performs USB SET_ADDRESS (BSR=true leaves Default / addr 0).
                    match ctrl.address_device(slot_id, port, speed, false) {
                        Ok(()) => {
                            crate::serial_println!(
                                "[xhci] Address Device OK (port {}, speed {:?})",
                                port,
                                speed
                            );
                            // FS: discover the real EP0 max packet. LS: MPS is
                            // always 8, but the gentle 8-byte-first read also
                            // unsticks quirky devices that stall an immediate
                            // full-length request (Athena port 4).
                            if matches!(speed, PortSpeed::FullSpeed | PortSpeed::LowSpeed) {
                                ctrl.ensure_fs_ep0_mps(slot_id);
                            }
                            match ctrl.get_device_descriptor_with_recovery(slot_id) {
                                Ok(desc) => {
                                    let bcd = u16::from_le_bytes([desc[2], desc[3]]);
                                    let vendor = u16::from_le_bytes([desc[8], desc[9]]);
                                    let product = u16::from_le_bytes([desc[10], desc[11]]);
                                    crate::serial_println!(
                                        "[xhci] Device descriptor: bLength={} bDescriptorType={} bcdUSB={:04x} idVendor={:04x} idProduct={:04x} bDeviceClass={}",
                                        desc[0],
                                        desc[1],
                                        bcd,
                                        vendor,
                                        product,
                                        desc[4],
                                    );
                                    if desc[4] == 0x09 {
                                        // Hub on a root port — DEFER enumeration
                                        // until every direct device is up (see
                                        // deferred_hubs rationale above).
                                        crate::serial_println!(
                                            "[usb-hub] slot {} on port {} is a hub — deferring enumeration",
                                            slot_id,
                                            port
                                        );
                                        deferred_hubs.push((slot_id, port, desc[6]));
                                    } else {
                                        // Classify by the configuration descriptor (USB
                                        // Audio → MSC → HID) and bring the device up —
                                        // shared with the hub-child path so every class
                                        // works at every attachment point.
                                        match ctrl
                                            .classify_and_bring_up(slot_id, vendor, product, "xhci")
                                        {
                                            BroughtUp::Hid => direct_hid_brought_up += 1,
                                            BroughtUp::Msc => direct_msc_brought_up += 1,
                                            _ => {}
                                        }
                                    }
                                }
                                Err(e) => {
                                    crate::serial_println!(
                                        "[xhci] GET_DESCRIPTOR (device) failed: {:?}",
                                        e
                                    );
                                }
                            }
                        }
                        Err(e) => crate::serial_println!("[xhci] Address Device failed: {:?}", e),
                    }
                }
                Err(e) => crate::serial_println!("[xhci] Enable Slot failed: {:?}", e),
            }
            // Bring up every connected port — QEMU exposes usb-tablet (port 5)
            // and usb-kbd (port 6); the boot keyboard is the device we need for
            // the live HID report path, so do not stop after the first device.
        }
    }

    // Now enumerate the deferred hubs. Skip them on REAL hardware when direct
    // HID devices already came up: hub enumeration can HCE-halt the controller
    // (Athena), which would kill the keyboard/mouse we just brought up. QEMU
    // never halts there and its coverage includes a keyboard behind a hub, so
    // always enumerate hubs under QEMU. On bare metal with zero direct HID we
    // still try — the hub may be the only path to an input device.
    let is_qemu = cpuid_hv_vendor_is_qemu();
    // Skip hubs on bare metal ONLY when both input AND storage already came up
    // directly. The old policy (skip whenever direct HID was up) protected the
    // keyboard from a hub-induced controller halt — but it also made a boot
    // stick in a hub-routed (front) port permanently invisible, which costs us
    // the persisted BOOTLOG on Athena. The hub-halt root causes are fixed
    // (USB3 0x2A descriptor, SET_HUB_DEPTH timeout recovery, EP0 stall
    // recovery), so when storage is still missing the hubs must be searched.
    let skip_hubs = !is_qemu && direct_hid_brought_up > 0 && direct_msc_brought_up > 0;
    HUBS_DEFERRED_TOTAL.fetch_add(deferred_hubs.len(), core::sync::atomic::Ordering::Relaxed);
    if skip_hubs {
        HUBS_SKIPPED_TOTAL.fetch_add(deferred_hubs.len(), core::sync::atomic::Ordering::Relaxed);
        crate::serial_println!(
            "[usb-hub] skipping {} deferred hub(s): {} HID + {} MSC already up direct, not risking a controller halt on bare metal",
            deferred_hubs.len(),
            direct_hid_brought_up,
            direct_msc_brought_up
        );
    } else {
        if !is_qemu && !deferred_hubs.is_empty() && direct_hid_brought_up > 0 {
            crate::serial_println!(
                "[usb-hub] no direct MSC device — enumerating {} deferred hub(s) to look for the boot stick",
                deferred_hubs.len()
            );
        }
        for (hub_slot, root_port, hub_protocol) in deferred_hubs.iter().copied() {
            crate::serial_println!(
                "[usb-hub] enumerating deferred hub slot {} (port {})",
                hub_slot,
                root_port
            );
            if let Err(e) = ctrl.enumerate_hub(hub_slot, 0, 1, root_port, hub_protocol) {
                crate::serial_println!("[usb-hub] root-port hub enum failed: {:?}", e);
            }
        }
    }

    // Arm one interrupt-IN per HID endpoint, then hand off to the servicing
    // thread so live keystrokes/pointer reports flow into input::push_event.
    let armed = ctrl.arm_hid_interrupts();
    crate::serial_println!(
        "[xhci] armed {} HID interrupt-IN endpoint(s) for live input",
        armed
    );
    armed
}

// ─── Live HID Input Servicing Thread ──────────────────────────────────────────

extern "C" fn hid_input_thread_entry() {
    // One-shot start marker: captured by the late bootlog flush so we can tell
    // off-target whether this thread is actually scheduled on iron. If it's
    // missing while a mouse is 'armed', the armed endpoint is fine but nothing
    // is draining its reports (post-boot scheduling problem), explaining a
    // dead cursor.
    crate::serial_println!(
        "[xhci] HID input thread started (draining armed interrupt-IN endpoints)"
    );
    // HID-DIAG: heartbeat so a bootlog reveals, off-target, whether this thread
    // is actually scheduled AND whether ANY interrupt-IN reports are arriving.
    // A dead cursor with reports=0 means the armed endpoint isn't DELIVERING
    // (arm/doorbell/interval problem); reports>0 with a dead cursor means the
    // report→cursor routing/parse is the bug (e.g. a mouse classified as kbd).
    // Logged to the RAM ring; captured by the (now-extended) late flush.
    let mut iters: u64 = 0;
    let mut reports: u64 = 0;
    let mut last_beat: u64 = 0;
    let mut flushed_first = false;
    loop {
        iters = iters.wrapping_add(1);
        let reports_before = reports;
        {
            let mut ctrl = XHCI_CONTROLLER.lock();
            if ctrl.initialized {
                reports = reports.wrapping_add(ctrl.service_hid_reports() as u64);
            }
        }
        {
            // Drain every secondary controller's HID completions too — a
            // keyboard on c6:00.3 delivers reports to THAT xHC's event ring.
            let mut secondaries = XHCI_SECONDARY.lock();
            for ctrl in secondaries.iter_mut() {
                if ctrl.initialized {
                    reports = reports.wrapping_add(ctrl.service_hid_reports() as u64);
                }
            }
        }
        // Slow-keys tick (Phase 19.3): a key held past its accessibility dwell
        // emerges here. A no-op (empty) unless a user enabled slow keys, so this
        // adds nothing to the hot path by default.
        for (code, down) in crate::a11y_input::poll(crate::aurora::aurora_now_ms()) {
            crate::input::push_event(
                0,
                if down {
                    crate::input::InputEventType::KeyDown(code)
                } else {
                    crate::input::InputEventType::KeyUp(code)
                },
            );
        }
        // ONE-SHOT flush the instant the FIRST report arrives, so a single mouse
        // wiggle / keypress is persisted to BOOTLOG.TXT regardless of when the
        // user powers off (the periodic + ~600-tick timer flushes close too early
        // to reliably catch post-desktop input). Bounded (once) + user-triggered +
        // well past bring-up, so the "no block I/O in hot threads" hazard (a flush
        // DURING bring-up or in a tight loop) does not apply.
        if reports > 0 && !flushed_first {
            flushed_first = true;
            crate::serial_println!(
                "[hid-diag] FIRST HID REPORT received (reports={}) — flushing BOOTLOG.TXT to capture it",
                reports
            );
            crate::bootlog_persist::flush();
        }
        // SPARSE liveness heartbeat, throttled by ITERATION COUNT (timer-rate
        // independent). The old `JIFFIES + 200` gate assumed 100 Hz, but iron
        // JIFFIES ticks ~16 kHz, so it fired ~80x/s and FLOODED the RAM ring with
        // 10k+ heartbeat lines — steamrolling the Tier-7..9 + userspace + amdgpud
        // bring-up transcript out of the 512 KiB ring before the stick was pulled
        // (iron bootlog 2026-06-22T2238: 10161 of 11265 lines were this heartbeat,
        // 0 amdgpu lines survived). The loop runs ~500 iters/s, so gating on 15000
        // iters logs ~every 30 s regardless of timer rate: a few liveness lines per
        // boot, leaving the bring-up transcript intact. Cheap serial only — NO
        // block I/O from this hot thread (that hung the desktop before).
        if iters.wrapping_sub(last_beat) >= 15_000 {
            last_beat = iters;
            crate::serial_println!(
                "[hid-diag] thread alive: iters={} reports={}",
                iters,
                reports
            );
        }
        // This thread is now a SCHED_GAME deadline (EDF) task (see
        // spawn_hid_input_thread): the scheduler picks it FIRST every ~2 ms
        // period, ahead of the whole Normal queue, so it no longer waits a full
        // normal-runqueue rotation to be serviced (the old ~8×/s / ~125 ms
        // batch latency — iron bootlog 2026-06-16T1651). yield_task() runs the
        // scheduler and, because this is a deadline task, calls dl.finish() so
        // the period is yielded to userspace until the next 2 ms boundary — no
        // busy-spin, no starvation. When reports are flowing, YIELD (re-picked
        // at the next period); when idle, HLT so the thread costs nothing until
        // the next timer tick wakes the period. Remaining latency floor is the
        // scheduler-entry rate (≥100 Hz timer); driving it lower needs MSI-X HID
        // wakeups (interrupt fires exactly when a report lands) — the documented
        // proper fix, a MasterChecklist follow-up beyond this EDF promotion.
        //
        // REGRESSION FIX (was 360b862): the idle branch MUST yield, not bare
        // `hlt()`. A bare `hlt` halts the CPU while this EDF task is STILL the
        // BSP's `current_task` and was never `finish()`ed for the period, so it
        // kept winning `pick_next` (the deadline class is evaluated before
        // Normal/CFS) and the BSP's entire Normal/CFS runqueue — the shell
        // auto-advance/autologin thread, init_system startup, the net poll
        // (DHCP), the thermal poll, the late bootlog flush — was permanently
        // starved (all 7 Normal tasks at last_cpu==u32::MAX; iron bootlog
        // 2026-06-16T1904 line 2136). `yield_task()` runs the scheduler and,
        // because this is a deadline task, calls `dl.finish()` so the period is
        // released to lower scheduling classes until the next 2 ms boundary.
        // The trailing `hlt()` (idle case only) then parks the CPU until the
        // next interrupt if yield_task handed us straight back — no busy-spin,
        // no starvation, the 2 ms input-latency win preserved.
        crate::scheduler::yield_task();
        if reports == reports_before {
            x86_64::instructions::hlt();
        }
    }
}

/// Spawn the kernel thread that drains HID interrupt-IN completions into the
/// input subsystem. Endpoints must already be armed (see `arm_hid_interrupts`).
pub fn spawn_hid_input_thread() {
    let mut task = crate::task::Task::new(hid_input_thread_entry, None);
    // SCHED_GAME deadline (EDF) task — input is a hot path. RaeenOS_Concept:
    // "Sub-frame input latency"; CLAUDE rule 4: "SCHED_GAME for hot paths …
    // not optional." As a plain Normal task the drain was only picked once per
    // FULL normal-runqueue rotation — ~8x/s (~125 ms batches) on Athena with
    // the desktop's many userspace tasks (iron bootlog 2026-06-16T1651:
    // hid-diag iters=8), so a moving mouse delivered position updates in ~125 ms
    // clumps (visible crawl + key-press lag) even with the 16-TD report buffer
    // (that fixed throughput, not pick latency). An EDF task is picked FIRST —
    // ahead of the entire Normal queue — every period, so drain latency is
    // bounded by a couple of scheduler ticks REGARDLESS of how many userspace
    // tasks exist, not by the rotation length. Same proven machinery the audio
    // thread uses (period 2667 us). Params: period 2 ms (the tightest cadence
    // the 1 ms-tick logical clock yields that still leaves alternating ticks for
    // userspace — a 1 ms period would be runnable every tick and starve the
    // Normal queue, the documented pick_next hazard), runtime 0.2 ms = 100
    // milli-cores, far under the 80% EDF admission gate even alongside audio.
    // Deadline-miss telemetry flows into /proc/raeen/gaming for free.
    task.priority = crate::task::TaskPriority::Game;
    task.deadline = Some(crate::task::DeadlineTask::new(2_000, 2_000, 200));
    let task_id = task.id;
    // Pin to CPU 0 — the APs don't schedule post-boot (see scheduler::spawn_on_bsp),
    // so a HID drain thread on an AP would never run and the mouse/keyboard would
    // be armed but dead. spawn_on_bsp sets affinity=CPU0 and keeps the deadline.
    crate::scheduler::spawn_on_bsp(task);
    crate::serial_println!(
        "[xhci] HID input servicing thread spawned (task={:?}, BSP-pinned, SCHED_GAME EDF 2ms)",
        task_id
    );
}

// ─── Global State ───────────────────────────────────────────────────────────

pub static XHCI_CONTROLLER: Mutex<XhciController> = Mutex::new(XhciController::new());

/// Controllers beyond the first. Real boards ship several xHCI functions —
/// Athena (AMD Phoenix) has FOUR (c4:00.3/.4, c6:00.3/.4), each owning a
/// different group of physical ports. Binding only the first left whole port
/// groups dead: a keyboard in a c6 port could never enumerate, and the boot
/// USB stick was invisible to USB-MSC (MasterChecklist 2.1). The primary
/// stays in [`XHCI_CONTROLLER`] so every existing single-controller consumer
/// is untouched; index 1.. live here.
pub static XHCI_SECONDARY: Mutex<Vec<XhciController>> = Mutex::new(Vec::new());

/// USB4/Thunderbolt host-router functions (prog_if 0x40) seen in the PCI
/// scan. We cannot tunnel USB3 through them, so a stick in a Type-C/USB4
/// port is invisible — the end-of-boot summary turns this into advice.
pub static USB4_FUNCTIONS_SEEN: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
/// xHCI functions whose `initialize()` failed (BAR/MMIO/reset problems).
pub static CONTROLLER_INIT_FAILURES: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
/// Hubs found across all controllers, and how many were skipped unprobed.
pub static HUBS_DEFERRED_TOTAL: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
pub static HUBS_SKIPPED_TOTAL: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
/// HID interrupt-IN endpoints armed across all controllers (set by
/// `run_boot_smoketest`).
pub static HID_ARMED_TOTAL: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

/// One-photo USB diagnosis: a compact block printed at the very END of boot
/// (so it survives in both the console tail and the curated diag panel's
/// most-recent window). Designed for the Athena flash cycle, where a single
/// photographed screen is the only evidence channel when the stick's
/// BOOTLOG.TXT could not be written.
pub fn print_end_of_boot_summary() {
    use core::sync::atomic::Ordering;

    let bound = controller_count();
    let failed = CONTROLLER_INIT_FAILURES.load(Ordering::Relaxed);
    let usb4 = USB4_FUNCTIONS_SEEN.load(Ordering::Relaxed);
    crate::serial_println!(
        "[usb-summary] controllers: {} bound, {} failed-init, {} USB4 fn(s){}",
        bound,
        failed,
        usb4,
        if usb4 > 0 {
            " (USB4/Type-C ports UNSUPPORTED — plug the stick into a USB-A port)"
        } else {
            ""
        },
    );
    for idx in 0..bound {
        let line = with_controller(idx, |c| {
            let mut connected = 0u32;
            for p in 1..=c.max_ports {
                if c.read_port_sc(p) & PORTSC_CCS != 0 {
                    connected += 1;
                }
            }
            (c.max_ports, connected, c.active_slot_count())
        });
        if let Some((ports, connected, slots)) = line {
            crate::serial_println!(
                "[usb-summary] ctrl{}: {} ports, {} connected, {} slot(s) enumerated",
                idx,
                ports,
                connected,
                slots
            );
        }
    }
    let msc = crate::usb_msc::MSC_DEVICES.lock().len();
    let hid = HID_ARMED_TOTAL.load(Ordering::Relaxed);
    let hubs = HUBS_DEFERRED_TOTAL.load(Ordering::Relaxed);
    let hubs_skipped = HUBS_SKIPPED_TOTAL.load(Ordering::Relaxed);
    crate::serial_println!(
        "[usb-summary] devices: MSC={} HID-armed={} hubs={} (skipped={}){}",
        msc,
        hid,
        hubs,
        hubs_skipped,
        if msc == 0 {
            " — NO STORAGE: stick missing/USB4 port/failed enumeration (see [xhci]/[usb-hub] above)"
        } else {
            ""
        },
    );
    crate::serial_println!(
        "[usb-summary] bootlog: {}",
        crate::bootlog_persist::status_line()
    );
}

/// Run `f` against controller `idx` (0 = primary, 1.. = secondaries, in
/// [`controller_count`] order). Returns `None` for an out-of-range index or
/// an uninitialized controller. The MSC layer stores this index per device
/// so bulk transfers reach the xHC that owns the slot.
pub fn with_controller<R>(idx: usize, f: impl FnOnce(&mut XhciController) -> R) -> Option<R> {
    if idx == 0 {
        let mut g = XHCI_CONTROLLER.lock();
        if g.initialized {
            Some(f(&mut g))
        } else {
            None
        }
    } else {
        let mut g = XHCI_SECONDARY.lock();
        match g.get_mut(idx - 1) {
            Some(c) if c.initialized => Some(f(c)),
            _ => None,
        }
    }
}

/// Number of bound controllers (primary + secondaries).
pub fn controller_count() -> usize {
    let primary = if XHCI_CONTROLLER.lock().initialized {
        1
    } else {
        0
    };
    primary + XHCI_SECONDARY.lock().len()
}

/// True once a controller passed `initialize()`. Used by the Tier-7 late
/// USB bring-up to decide whether the early (pre-ECAM, legacy-scan-only)
/// pass already found the controller. NOTE: on Athena the early pass sees
/// NO controllers (xHCI lives on ECAM-only buses), so the late pass runs
/// init() exactly once with every bus visible and binds all controllers in
/// one sweep — the hypothetical "early pass bound bus-0, late pass should
/// add ECAM-only extras" machine is not handled yet (would need per-BDF
/// bind tracking; no known target needs it).
pub fn is_initialized() -> bool {
    XHCI_CONTROLLER.lock().initialized
}

pub fn init() {
    crate::serial_println!("[xhci] scanning PCI for xHCI controllers...");
    let pci_devices = crate::pci::enumerate();

    // Panel-visible PCI environment summary (the on-screen diagnostic panel only
    // surfaces curated prefixes, so emit the key facts under [xhci] here).
    let ecam = crate::pci::PCIE_ECAM_BASE.load(core::sync::atomic::Ordering::Relaxed);
    crate::serial_println!(
        "[xhci] PCI scan: {} devices, ECAM={} (max_bus={})",
        pci_devices.len(),
        if ecam != 0 { "active" } else { "legacy" },
        if ecam != 0 { 255 } else { 8 },
    );
    // List every serial-bus / USB-class controller so we can tell whether the
    // xHCI is absent, on an unscanned bus, or presenting a non-xHCI prog_if.
    let mut usb_seen = 0;
    for dev in &pci_devices {
        if dev.class == 0x0C && dev.subclass == 0x03 {
            usb_seen += 1;
            let kind = match dev.prog_if {
                0x00 => "UHCI(USB1.1)",
                0x10 => "OHCI(USB1.1)",
                0x20 => "EHCI(USB2.0)",
                0x30 => "xHCI(USB3)",
                0x40 => {
                    // USB4/Thunderbolt host router — we cannot tunnel USB3
                    // through it. A stick in a Type-C/USB4 port is invisible;
                    // the end-of-boot summary tells the user to move it.
                    USB4_FUNCTIONS_SEEN.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    "USB4/TBT(unsupported — use a USB-A port)"
                }
                0xFE => "USB-device",
                _ => "USB-unknown",
            };
            crate::serial_println!(
                "[xhci] USB ctrl {:02x}:{:02x}.{} {:04x}:{:04x} prog_if={:#04x} {}",
                dev.bus,
                dev.device,
                dev.function,
                dev.vendor_id,
                dev.device_id,
                dev.prog_if,
                kind
            );
        }
    }
    if usb_seen == 0 {
        crate::serial_println!("[xhci] no USB controllers (class 0C/03) in PCI scan at all");
    }

    for dev in &pci_devices {
        if dev.class == 0x0C && dev.subclass == 0x03 && dev.prog_if == 0x30 {
            crate::pci::enable_bus_mastering(dev);
            if let Some(msix) = crate::pci::parse_msix_cap(dev) {
                crate::serial_println!(
                    "[pci] xHCI {:02x}:{:02x}.{} MSI-X: {} vectors bar{}+{:#x}",
                    dev.bus,
                    dev.device,
                    dev.function,
                    msix.table_size,
                    msix.table_bar,
                    msix.table_offset
                );
            } else if crate::pci::find_capability(dev, crate::pci::PCI_CAP_MSI).is_some() {
                crate::serial_println!(
                    "[pci] xHCI {:02x}:{:02x}.{} legacy MSI capability",
                    dev.bus,
                    dev.device,
                    dev.function
                );
            }
            let raw_bar0 = dev.bars[0];
            if raw_bar0 == 0 || raw_bar0 & 1 != 0 {
                crate::serial_println!(
                    "[xhci] controller {:02x}:{:02x}.{} BAR0={:#010x} unusable (zero/IO-type) — skipping",
                    dev.bus, dev.device, dev.function, raw_bar0
                );
                continue;
            }

            let is_64bit = (raw_bar0 >> 1) & 0x03 == 0x02;
            let bar_phys: u64 = if is_64bit {
                ((raw_bar0 as u64) & !0x0F) | ((dev.bars[1] as u64) << 32)
            } else {
                (raw_bar0 as u64) & !0x0F
            };
            if bar_phys == 0 {
                continue;
            }

            // 64-bit BARs can sit far above the linear physmap (e.g. 0x3800_0000_8000),
            // so we must create real MMIO page-table entries rather than assume
            // `offset + bar_phys` is already mapped. map_mmio_region returns the
            // physmap-consistent virtual address with caching disabled.
            let bar_size = {
                let s = crate::mmio::pci_bar_size_bytes(dev.bus, dev.device, dev.function, 0);
                if s == 0 {
                    0x10000
                } else {
                    s
                }
            };
            let mmio_base = crate::arch::mmu::kernel()
                .map_mmio_range(
                    x86_64::PhysAddr::new(bar_phys),
                    bar_size,
                    crate::arch::mmu::PageFlags::DEVICE,
                )
                .as_u64();

            crate::serial_println!(
                "[xhci] found controller at {:02x}:{:02x}.{} BAR0={:#010x} size={:#x} mmio_base={:#x}",
                dev.bus, dev.device, dev.function, bar_phys, bar_size, mmio_base
            );
            // First usable controller becomes the primary singleton (existing
            // consumers untouched); every further one is fully initialized
            // into XHCI_SECONDARY. Previously this `return`ed after the first
            // success — on Athena that bound c4:00.3 and left the other three
            // controllers (and all their physical ports) dead.
            let primary_free = !XHCI_CONTROLLER.lock().initialized;
            if primary_free {
                let mut ctrl = XHCI_CONTROLLER.lock();
                ctrl.pci_vendor_id = dev.vendor_id;
                match ctrl.initialize(mmio_base) {
                    Ok(()) => crate::serial_println!(
                        "[xhci] initialized (primary): {} slots, {} ports",
                        ctrl.max_slots,
                        ctrl.max_ports,
                    ),
                    Err(e) => {
                        CONTROLLER_INIT_FAILURES
                            .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                        crate::serial_println!("[xhci] init failed: {:?}", e)
                    }
                }
            } else {
                let mut ctrl = XhciController::new();
                ctrl.pci_vendor_id = dev.vendor_id;
                match ctrl.initialize(mmio_base) {
                    Ok(()) => {
                        crate::serial_println!(
                            "[xhci] initialized (secondary #{}): {} slots, {} ports",
                            XHCI_SECONDARY.lock().len() + 1,
                            ctrl.max_slots,
                            ctrl.max_ports,
                        );
                        XHCI_SECONDARY.lock().push(ctrl);
                    }
                    Err(e) => {
                        CONTROLLER_INIT_FAILURES
                            .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                        crate::serial_println!("[xhci] secondary controller init failed: {:?}", e)
                    }
                }
            }
        }
    }

    if !XHCI_CONTROLLER.lock().initialized {
        crate::serial_println!("[xhci] no xHCI controller found");
    }
}
