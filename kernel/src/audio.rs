#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

// ─── HDA Register Offsets ────────────────────────────────────────────────────

const HDA_GCAP: u32 = 0x00;
const HDA_VMIN: u32 = 0x02;
const HDA_VMAJ: u32 = 0x03;
const HDA_GCTL: u32 = 0x08;
const HDA_WAKEEN: u32 = 0x0C;
const HDA_STATESTS: u32 = 0x0E;
const HDA_INTCTL: u32 = 0x20;
const HDA_INTSTS: u32 = 0x24;
const HDA_CORBLBASE: u32 = 0x40;
const HDA_CORBUBASE: u32 = 0x44;
const HDA_CORBWP: u32 = 0x48;
const HDA_CORBRP: u32 = 0x4A;
const HDA_CORBCTL: u32 = 0x4C;
const HDA_CORBSIZE: u32 = 0x4E;
const HDA_RIRLBASE: u32 = 0x50;
const HDA_RIRUBASE: u32 = 0x54;
const HDA_RIRBWP: u32 = 0x58;
const HDA_RINTCNT: u32 = 0x5A;
const HDA_RIRBCTL: u32 = 0x5C;
const HDA_RIRBSTS: u32 = 0x5D;
const HDA_RIRBSIZE: u32 = 0x5E;

/// RIRBSTS response-interrupt flag (W1C). The controller PAUSES CORB
/// processing once RINTCNT responses are pending until this is acked —
/// QEMU enforces it strictly: without the ack only the FIRST verb of a
/// codec walk ever completes (CORBRP freezes at 1).
const RIRBSTS_RINTFL: u8 = 1 << 0;

const HDA_GCTL_CRST: u32 = 1 << 0;
const HDA_INTCTL_GIE: u32 = 1 << 31;
const HDA_INTCTL_CIE: u32 = 1 << 30;

// Stream descriptor offsets (relative to 0x80 + stream_index * 0x20)
const HDA_SD_BASE: u32 = 0x80;
const HDA_SD_STRIDE: u32 = 0x20;
const HDA_SD_CTL: u32 = 0x00;
const HDA_SD_STS: u32 = 0x03;
const HDA_SD_LPIB: u32 = 0x04;
const HDA_SD_CBL: u32 = 0x08;
const HDA_SD_LVI: u32 = 0x0C;
const HDA_SD_FMT: u32 = 0x12;
const HDA_SD_BDLPL: u32 = 0x18;
const HDA_SD_BDLPU: u32 = 0x1C;

const HDA_SD_CTL_SRST: u32 = 1 << 0;
const HDA_SD_CTL_RUN: u32 = 1 << 1;
const HDA_SD_CTL_IOCE: u32 = 1 << 2;
const HDA_SD_CTL_DEIE: u32 = 1 << 4;

const CORB_RUN: u8 = 1 << 1;
const RIRB_DMAEN: u8 = 1 << 1;
/// RIRBCTL response-interrupt enable. REQUIRED even on a polled driver:
/// the controller only raises RIRBSTS.RINTFL when this is set, and CORB
/// processing stays frozen after RINTCNT responses until that flag is
/// acked — without IRQ_EN the flag never sets, the ack never "clears"
/// anything, and the engine wedges after the FIRST verb of a codec walk.
const RIRB_IRQ_EN: u8 = 1 << 0;

// Target: 128 frames @ 48 kHz = ~2.67 ms per period
const AUDIO_PERIOD_FRAMES: usize = 128;
const AUDIO_CHANNELS: usize = 2;
const AUDIO_PERIOD_SAMPLES: usize = AUDIO_PERIOD_FRAMES * AUDIO_CHANNELS;
const RING_BUFFER_SAMPLES: usize = 4096;
const BDL_ENTRIES: usize = 4;

// ─── Physical-to-virtual helper ─────────────────────────────────────────────

fn phys_to_virt(phys: u64) -> u64 {
    let offset = crate::memory::PHYS_MEM_OFFSET
        .get()
        .expect("PHYS_MEM_OFFSET not initialized");
    offset.as_u64().wrapping_add(phys)
}

// ─── HDA Verb Construction ──────────────────────────────────────────────────

/// 12-bit verb ID + 8-bit payload (e.g. GET_PARAMETER = 0xF00 + param_id)
const fn hda_verb(codec: u8, nid: u8, verb: u16, payload: u8) -> u32 {
    ((codec as u32) << 28) | ((nid as u32) << 20) | ((verb as u32) << 8) | (payload as u32)
}

/// 4-bit verb ID + 16-bit payload (e.g. SET verbs)
const fn hda_verb_12(codec: u8, nid: u8, verb: u8, payload: u16) -> u32 {
    ((codec as u32) << 28) | ((nid as u32) << 20) | ((verb as u32 & 0xF) << 16) | (payload as u32)
}

// ─── HDA Types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdaState {
    Uninitialized,
    Reset,
    Running,
    Suspended,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdaNodeType {
    AudioOutput,
    AudioInput,
    AudioMixer,
    AudioSelector,
    PinComplex,
    VolumeKnob,
    BeepGenerator,
    VendorDefined,
}

impl HdaNodeType {
    fn from_widget_type(wtype: u8) -> Self {
        match wtype {
            0x0 => HdaNodeType::AudioOutput,
            0x1 => HdaNodeType::AudioInput,
            0x2 => HdaNodeType::AudioMixer,
            0x3 => HdaNodeType::AudioSelector,
            0x4 => HdaNodeType::PinComplex,
            0x6 => HdaNodeType::VolumeKnob,
            0x7 => HdaNodeType::BeepGenerator,
            _ => HdaNodeType::VendorDefined,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamDirection {
    Playback,
    Capture,
}

#[derive(Debug, Clone)]
pub struct HdaNode {
    pub nid: u8,
    pub node_type: HdaNodeType,
    pub capabilities: u32,
    pub config_default: u32,
    pub connection_list: Vec<u8>,
}

impl HdaNode {
    pub fn is_output_pin(&self) -> bool {
        if self.node_type != HdaNodeType::PinComplex {
            return false;
        }
        let default_device = (self.config_default >> 20) & 0xF;
        matches!(default_device, 0x0 | 0x1 | 0x3 | 0x4)
    }

    pub fn is_input_pin(&self) -> bool {
        if self.node_type != HdaNodeType::PinComplex {
            return false;
        }
        let default_device = (self.config_default >> 20) & 0xF;
        matches!(default_device, 0x8 | 0xA)
    }
}

#[derive(Debug, Clone)]
pub struct HdaCodec {
    pub codec_id: u8,
    pub vendor_id: u32,
    pub subsystem_id: u32,
    pub nodes: Vec<HdaNode>,
    pub afg_node: u8,
}

impl HdaCodec {
    pub fn vendor_name(&self) -> &'static str {
        match (self.vendor_id >> 16) as u16 {
            0x10DE => "NVIDIA",
            0x8086 => "Intel",
            0x1002 => "AMD",
            0x10EC => "Realtek",
            0x11D4 => "Analog Devices",
            0x14F1 => "Conexant",
            _ => "Unknown",
        }
    }

    pub fn find_dac(&self) -> Option<&HdaNode> {
        self.nodes
            .iter()
            .find(|n| n.node_type == HdaNodeType::AudioOutput)
    }

    pub fn find_adc(&self) -> Option<&HdaNode> {
        self.nodes
            .iter()
            .find(|n| n.node_type == HdaNodeType::AudioInput)
    }

    pub fn output_pins(&self) -> Vec<&HdaNode> {
        self.nodes.iter().filter(|n| n.is_output_pin()).collect()
    }

    pub fn input_pins(&self) -> Vec<&HdaNode> {
        self.nodes.iter().filter(|n| n.is_input_pin()).collect()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AudioStreamFormat {
    pub sample_rate: u32,
    pub bits_per_sample: u8,
    pub channels: u8,
}

impl AudioStreamFormat {
    pub const CD_QUALITY: Self = Self {
        sample_rate: 44100,
        bits_per_sample: 16,
        channels: 2,
    };
    pub const DVD_QUALITY: Self = Self {
        sample_rate: 48000,
        bits_per_sample: 16,
        channels: 2,
    };
    pub const STUDIO: Self = Self {
        sample_rate: 96000,
        bits_per_sample: 24,
        channels: 2,
    };
    pub const HIRES: Self = Self {
        sample_rate: 192000,
        bits_per_sample: 32,
        channels: 2,
    };

    pub fn bytes_per_frame(&self) -> u32 {
        (self.bits_per_sample as u32 / 8) * self.channels as u32
    }

    pub fn bytes_per_second(&self) -> u32 {
        self.bytes_per_frame() * self.sample_rate
    }

    pub fn to_hda_format_register(&self) -> u16 {
        let base = match self.sample_rate {
            44100 => 1u16 << 14,
            _ => 0u16,
        };
        let mult = match self.sample_rate {
            96000 => 1u16 << 11,
            192000 => 3u16 << 11,
            _ => 0u16,
        };
        let bits = match self.bits_per_sample {
            16 => 1u16 << 4,
            20 => 2u16 << 4,
            24 => 3u16 << 4,
            32 => 4u16 << 4,
            _ => 0u16,
        };
        let chans = (self.channels.saturating_sub(1) as u16) & 0xF;
        base | mult | bits | chans
    }
}

// ─── HDA BDL Entry (DMA descriptor) ────────────────────────────────────────

/// Buffer Descriptor List entry — laid out exactly as the HDA controller expects.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HdaBdlEntry {
    pub address: u64,
    pub length: u32,
    pub flags: u32, // bit 0 = IOC (Interrupt on Completion)
}

#[derive(Debug, Clone, Copy)]
pub struct BufferDescriptor {
    pub address: u64,
    pub length: u32,
    pub ioc: bool,
}

#[derive(Debug, Clone)]
pub struct HdaStream {
    pub stream_id: u8,
    pub direction: StreamDirection,
    pub format: AudioStreamFormat,
    pub bdl: Vec<BufferDescriptor>,
    pub running: bool,
    pub position: u64,
    pub bdl_phys: u64,
    pub dma_phys: u64,
    pub dma_virt: u64,
    pub dma_size: u32,
}

impl HdaStream {
    pub fn new(id: u8, direction: StreamDirection, format: AudioStreamFormat) -> Self {
        Self {
            stream_id: id,
            direction,
            format,
            bdl: Vec::new(),
            running: false,
            position: 0,
            bdl_phys: 0,
            dma_phys: 0,
            dma_virt: 0,
            dma_size: 0,
        }
    }

    pub fn add_buffer(&mut self, address: u64, length: u32, ioc: bool) {
        self.bdl.push(BufferDescriptor {
            address,
            length,
            ioc,
        });
    }

    pub fn total_buffer_bytes(&self) -> u64 {
        self.bdl.iter().map(|b| b.length as u64).sum()
    }

    pub fn latency_us(&self) -> u64 {
        let bytes_per_sec = self.format.bytes_per_second() as u64;
        if bytes_per_sec == 0 {
            return 0;
        }
        (self.total_buffer_bytes() * 1_000_000) / bytes_per_sec
    }
}

// ─── CORB / RIRB DMA Buffers ────────────────────────────────────────────────

#[derive(Debug)]
pub struct CorbBuffer {
    pub phys: u64,
    pub virt: u64,
    pub size: u16,
    pub write_ptr: u16,
}

#[derive(Debug)]
pub struct RirbBuffer {
    pub phys: u64,
    pub virt: u64,
    pub size: u16,
    pub write_ptr: u16,
}

// ─── HDA Controller ────────────────────────────────────────────────────────

pub struct HdaController {
    pub base_phys: u64,
    pub mmio_base: u64, // virtual address for MMIO access
    pub codecs: Vec<HdaCodec>,
    pub corb: CorbBuffer,
    pub rirb: RirbBuffer,
    pub streams: Vec<HdaStream>,
    pub state: HdaState,
    pub num_output_streams: u8,
    pub num_input_streams: u8,
    pub num_bidir_streams: u8,
}

impl HdaController {
    pub fn new(base_phys: u64) -> Self {
        let mmio_base = phys_to_virt(base_phys);
        Self {
            base_phys,
            mmio_base,
            codecs: Vec::new(),
            corb: CorbBuffer {
                phys: 0,
                virt: 0,
                size: 256,
                write_ptr: 0,
            },
            rirb: RirbBuffer {
                phys: 0,
                virt: 0,
                size: 256,
                write_ptr: 0,
            },
            streams: Vec::new(),
            state: HdaState::Uninitialized,
            num_output_streams: 0,
            num_input_streams: 0,
            num_bidir_streams: 0,
        }
    }

    // ── MMIO primitives ─────────────────────────────────────────────────

    unsafe fn mmio_read32(&self, offset: u32) -> u32 {
        let ptr = (self.mmio_base + offset as u64) as *const u32;
        core::ptr::read_volatile(ptr)
    }

    unsafe fn mmio_write32(&self, offset: u32, val: u32) {
        let ptr = (self.mmio_base + offset as u64) as *mut u32;
        core::ptr::write_volatile(ptr, val);
    }

    unsafe fn mmio_read16(&self, offset: u32) -> u16 {
        let ptr = (self.mmio_base + offset as u64) as *const u16;
        core::ptr::read_volatile(ptr)
    }

    unsafe fn mmio_write16(&self, offset: u32, val: u16) {
        let ptr = (self.mmio_base + offset as u64) as *mut u16;
        core::ptr::write_volatile(ptr, val);
    }

    unsafe fn mmio_read8(&self, offset: u32) -> u8 {
        let ptr = (self.mmio_base + offset as u64) as *const u8;
        core::ptr::read_volatile(ptr)
    }

    unsafe fn mmio_write8(&self, offset: u32, val: u8) {
        let ptr = (self.mmio_base + offset as u64) as *mut u8;
        core::ptr::write_volatile(ptr, val);
    }

    fn stream_reg_base(&self, stream_index: u8) -> u32 {
        HDA_SD_BASE + (stream_index as u32) * HDA_SD_STRIDE
    }

    // ── Controller initialisation (Intel HDA §3) ───────────────────────

    /// Full init sequence: GCAP → reset → CORB/RIRB → codec discovery → interrupts.
    pub fn init_controller(&mut self) -> Result<(), &'static str> {
        // 1. Read Global Capabilities
        let gcap = unsafe { self.mmio_read16(HDA_GCAP) };
        self.num_output_streams = ((gcap >> 12) & 0xF) as u8;
        self.num_input_streams = ((gcap >> 8) & 0xF) as u8;
        self.num_bidir_streams = ((gcap >> 3) & 0x1F) as u8;

        crate::serial_println!(
            "[audio] GCAP={:#06x}  out={} in={} bidir={}",
            gcap,
            self.num_output_streams,
            self.num_input_streams,
            self.num_bidir_streams
        );

        // 2. Assert CRST in GCTL, wait for codec detection
        self.reset_controller()?;

        // 3. Allocate + program CORB & RIRB DMA rings
        self.setup_corb_rirb()?;

        // 4. Discover codecs via STATESTS
        let statests = unsafe { self.mmio_read16(HDA_STATESTS) };
        crate::serial_println!("[audio] STATESTS={:#06x}", statests);
        for addr in 0u8..15 {
            if statests & (1 << addr) != 0 {
                match self.discover_codec(addr) {
                    Ok(()) => crate::serial_println!("[audio] codec {} discovered", addr),
                    Err(e) => {
                        crate::serial_println!("[audio] codec {} discovery failed: {}", addr, e)
                    }
                }
            }
        }
        // Clear STATESTS by writing ones to the bits that are set
        unsafe {
            self.mmio_write16(HDA_STATESTS, statests);
        }

        // 5. Enable global + controller interrupts
        unsafe {
            self.mmio_write32(HDA_INTCTL, HDA_INTCTL_GIE | HDA_INTCTL_CIE);
        }

        self.state = HdaState::Running;
        Ok(())
    }

    /// Assert and de-assert controller reset via GCTL.CRST.
    fn reset_controller(&mut self) -> Result<(), &'static str> {
        unsafe {
            // Enter reset: clear CRST
            let gctl = self.mmio_read32(HDA_GCTL);
            self.mmio_write32(HDA_GCTL, gctl & !HDA_GCTL_CRST);

            // Wait for CRST to read back as 0
            for _ in 0..1_000 {
                if self.mmio_read32(HDA_GCTL) & HDA_GCTL_CRST == 0 {
                    break;
                }
                core::hint::spin_loop();
            }

            // Exit reset: set CRST
            let gctl = self.mmio_read32(HDA_GCTL);
            self.mmio_write32(HDA_GCTL, gctl | HDA_GCTL_CRST);

            // Wait for CRST to read back as 1
            for _ in 0..1_000 {
                if self.mmio_read32(HDA_GCTL) & HDA_GCTL_CRST != 0 {
                    self.state = HdaState::Reset;

                    // Codecs need time to enumerate after reset (HDA spec §4.3).
                    // Spin briefly; a proper driver would use a timer callback.
                    for _ in 0..100_000 {
                        core::hint::spin_loop();
                    }
                    return Ok(());
                }
                core::hint::spin_loop();
            }
        }
        self.state = HdaState::Error;
        Err("HDA controller reset timeout")
    }

    /// Allocate page-aligned DMA buffers for CORB (256×4 B) and RIRB (256×8 B),
    /// program the controller registers, and start both engines.
    fn setup_corb_rirb(&mut self) -> Result<(), &'static str> {
        use x86_64::structures::paging::FrameAllocator;
        let mut alloc = crate::memory::GlobalFrameAllocator;

        // ── CORB: 256 entries × 4 bytes = 1 KiB (fits in one 4 KiB page) ──
        let corb_frame = alloc
            .allocate_frame()
            .ok_or("failed to allocate CORB DMA page")?;
        let corb_phys = corb_frame.start_address().as_u64();
        let corb_virt = phys_to_virt(corb_phys);
        unsafe {
            core::ptr::write_bytes(corb_virt as *mut u8, 0, 4096);
        }

        // ── RIRB: 256 entries × 8 bytes = 2 KiB ────────────────────────────
        let rirb_frame = alloc
            .allocate_frame()
            .ok_or("failed to allocate RIRB DMA page")?;
        let rirb_phys = rirb_frame.start_address().as_u64();
        let rirb_virt = phys_to_virt(rirb_phys);
        unsafe {
            core::ptr::write_bytes(rirb_virt as *mut u8, 0, 4096);
        }

        unsafe {
            // Stop both engines while we reconfigure
            self.mmio_write8(HDA_CORBCTL, 0);
            self.mmio_write8(HDA_RIRBCTL, 0);

            // ── CORB registers ──────────────────────────────────────────
            self.mmio_write32(HDA_CORBLBASE, corb_phys as u32);
            self.mmio_write32(HDA_CORBUBASE, (corb_phys >> 32) as u32);
            self.mmio_write8(HDA_CORBSIZE, 0x02); // 256 entries

            // Reset read pointer (set bit 15, wait, clear)
            self.mmio_write16(HDA_CORBRP, 0x8000);
            for _ in 0..1_000 {
                if self.mmio_read16(HDA_CORBRP) & 0x8000 != 0 {
                    break;
                }
                core::hint::spin_loop();
            }
            self.mmio_write16(HDA_CORBRP, 0x0000);
            for _ in 0..1_000 {
                if self.mmio_read16(HDA_CORBRP) & 0x8000 == 0 {
                    break;
                }
                core::hint::spin_loop();
            }

            // Write pointer starts at 0
            self.mmio_write16(HDA_CORBWP, 0);

            // ── RIRB registers ──────────────────────────────────────────
            self.mmio_write32(HDA_RIRLBASE, rirb_phys as u32);
            self.mmio_write32(HDA_RIRUBASE, (rirb_phys >> 32) as u32);
            self.mmio_write8(HDA_RIRBSIZE, 0x02); // 256 entries

            // Reset RIRB write pointer
            self.mmio_write16(HDA_RIRBWP, 0x8000);

            // Interrupt after every response
            self.mmio_write16(HDA_RINTCNT, 1);

            // Start both engines. IRQ_EN must accompany DMAEN even though
            // send_verb polls — see RIRB_IRQ_EN: the response-count
            // backpressure only releases through a set+acked RINTFL.
            self.mmio_write8(HDA_CORBCTL, CORB_RUN);
            self.mmio_write8(HDA_RIRBCTL, RIRB_DMAEN | RIRB_IRQ_EN);
        }

        self.corb = CorbBuffer {
            phys: corb_phys,
            virt: corb_virt,
            size: 256,
            write_ptr: 0,
        };
        self.rirb = RirbBuffer {
            phys: rirb_phys,
            virt: rirb_virt,
            size: 256,
            write_ptr: 0,
        };
        Ok(())
    }

    /// Submit a verb to the CORB and spin-wait for the RIRB response.
    fn send_verb(&mut self, verb: u32) -> Result<u32, &'static str> {
        let wp = (self.corb.write_ptr + 1) % self.corb.size;

        // Write the verb into the next CORB slot
        unsafe {
            let slot = (self.corb.virt + (wp as u64) * 4) as *mut u32;
            core::ptr::write_volatile(slot, verb);
        }
        self.corb.write_ptr = wp;

        // Advance the hardware write pointer so the DMA engine fetches it
        unsafe {
            self.mmio_write16(HDA_CORBWP, wp);
        }

        // Spin-wait for a RIRB response
        for _ in 0..100_000 {
            let hw_wp = unsafe { self.mmio_read16(HDA_RIRBWP) } & 0xFF;
            while self.rirb.write_ptr != hw_wp {
                self.rirb.write_ptr = (self.rirb.write_ptr + 1) % self.rirb.size;
                // Each RIRB entry is 8 bytes: [31:0] response, [63:32] extended
                let (response, extended) = unsafe {
                    let slot = (self.rirb.virt + (self.rirb.write_ptr as u64) * 8) as *const u32;
                    (
                        core::ptr::read_volatile(slot),
                        core::ptr::read_volatile(slot.add(1)),
                    )
                };

                // Ack the response interrupt (W1C) — with RINTCNT=1 the
                // controller stops fetching further CORB entries until the
                // pending response is acknowledged.
                unsafe {
                    self.mmio_write8(HDA_RIRBSTS, RIRBSTS_RINTFL);
                }

                if (extended & (1 << 4)) != 0 {
                    // Unsolicited response, ignore for now to avoid desyncing the command queue
                    continue;
                }

                return Ok(response);
            }
            core::hint::spin_loop();
        }
        // One diagnostic snapshot on the first timeout: enough to tell a
        // dead DMA engine (CTL bits clear) from a stuck pointer (RP not
        // advancing) from a response that landed where we don't look.
        unsafe {
            crate::serial_println!(
                "[audio] verb timeout diag: GCTL={:#x} CORBCTL={:#x} CORBRP={:#x} CORBWP={:#x} RIRBCTL={:#x} RIRBWP={:#x} sw_corb_wp={} sw_rirb_wp={}",
                self.mmio_read32(HDA_GCTL),
                self.mmio_read8(HDA_CORBCTL),
                self.mmio_read16(HDA_CORBRP),
                self.mmio_read16(HDA_CORBWP),
                self.mmio_read8(HDA_RIRBCTL),
                self.mmio_read16(HDA_RIRBWP),
                self.corb.write_ptr,
                self.rirb.write_ptr,
            );
        }
        Err("RIRB response timeout")
    }

    /// Walk a codec's function-group tree to discover widgets.
    fn discover_codec(&mut self, codec_addr: u8) -> Result<(), &'static str> {
        // Root node (NID 0): GET_PARAMETER → Vendor ID
        let vendor_id = self.send_verb(hda_verb(codec_addr, 0, 0xF00, 0x00))?;

        // Root node: GET_PARAMETER → Subordinate Node Count
        let sub_resp = self.send_verb(hda_verb(codec_addr, 0, 0xF00, 0x04))?;
        let start_nid = ((sub_resp >> 16) & 0xFF) as u8;
        let num_fg = (sub_resp & 0xFF) as u8;

        // Find the Audio Function Group (type 0x01)
        let mut afg_nid = start_nid;
        for offset in 0..num_fg {
            let nid = start_nid + offset;
            let fg_type = self.send_verb(hda_verb(codec_addr, nid, 0xF00, 0x05))?;
            if fg_type & 0xFF == 0x01 {
                afg_nid = nid;
                break;
            }
        }

        // AFG subordinate nodes (the actual widgets)
        let afg_sub = self.send_verb(hda_verb(codec_addr, afg_nid, 0xF00, 0x04))?;
        let widget_start = ((afg_sub >> 16) & 0xFF) as u8;
        let widget_count = (afg_sub & 0xFF) as u8;

        let mut codec = HdaCodec {
            codec_id: codec_addr,
            vendor_id,
            subsystem_id: 0,
            nodes: Vec::new(),
            afg_node: afg_nid,
        };

        for i in 0..widget_count {
            let nid = widget_start + i;
            let caps = self.send_verb(hda_verb(codec_addr, nid, 0xF00, 0x09))?;
            let wtype = ((caps >> 20) & 0xF) as u8;

            let config = if wtype == 4 {
                self.send_verb(hda_verb(codec_addr, nid, 0xF1C, 0x00))?
            } else {
                0
            };

            codec.nodes.push(HdaNode {
                nid,
                node_type: HdaNodeType::from_widget_type(wtype),
                capabilities: caps,
                config_default: config,
                connection_list: Vec::new(),
            });
        }

        self.codecs.push(codec);
        Ok(())
    }

    // ── Stream setup / control ──────────────────────────────────────────

    /// Allocate DMA buffers, build BDL, and program the hardware stream descriptor.
    pub fn setup_stream(
        &mut self,
        direction: StreamDirection,
        format: AudioStreamFormat,
    ) -> Result<u8, &'static str> {
        use x86_64::structures::paging::FrameAllocator;
        let mut alloc = crate::memory::GlobalFrameAllocator;

        let id = self.streams.len() as u8;
        let mut stream = HdaStream::new(id, direction, format);

        let bytes_per_period = (AUDIO_PERIOD_FRAMES
            * format.channels as usize
            * (format.bits_per_sample as usize / 8)) as u32;

        // Allocate one page for the BDL itself (BDL_ENTRIES × 16 bytes)
        let bdl_frame = alloc
            .allocate_frame()
            .ok_or("failed to allocate BDL page")?;
        let bdl_phys = bdl_frame.start_address().as_u64();
        let bdl_virt = phys_to_virt(bdl_phys);
        unsafe {
            core::ptr::write_bytes(bdl_virt as *mut u8, 0, 4096);
        }

        // Allocate one page for the audio DMA data buffers
        let dma_frame = alloc
            .allocate_frame()
            .ok_or("failed to allocate DMA audio page")?;
        let dma_phys = dma_frame.start_address().as_u64();
        let dma_virt = phys_to_virt(dma_phys);
        unsafe {
            core::ptr::write_bytes(dma_virt as *mut u8, 0, 4096);
        }

        let total_bytes = bytes_per_period * BDL_ENTRIES as u32;
        stream.bdl_phys = bdl_phys;
        stream.dma_phys = dma_phys;
        stream.dma_virt = dma_virt;
        stream.dma_size = total_bytes;

        // Fill BDL entries — each points to a slice of the DMA page
        for i in 0..BDL_ENTRIES {
            let buf_offset = (i as u32) * bytes_per_period;
            let entry_addr = bdl_virt + (i as u64) * 16;
            let ioc = i == BDL_ENTRIES - 1; // IOC on last entry
            let entry = HdaBdlEntry {
                address: dma_phys + buf_offset as u64,
                length: bytes_per_period,
                flags: if ioc { 1 } else { 0 },
            };
            unsafe {
                core::ptr::write_volatile(entry_addr as *mut HdaBdlEntry, entry);
            }
            stream.add_buffer(dma_phys + buf_offset as u64, bytes_per_period, ioc);
        }

        // Program the hardware stream descriptor registers
        let base = self.stream_reg_base(id);
        unsafe {
            // Reset the stream
            let ctl = self.mmio_read32(base + HDA_SD_CTL) & 0x00FF_FFFF;
            self.mmio_write32(base + HDA_SD_CTL, ctl | HDA_SD_CTL_SRST);
            for _ in 0..1_000 {
                if self.mmio_read32(base + HDA_SD_CTL) & HDA_SD_CTL_SRST != 0 {
                    break;
                }
                core::hint::spin_loop();
            }
            self.mmio_write32(base + HDA_SD_CTL, ctl & !HDA_SD_CTL_SRST);
            for _ in 0..1_000 {
                if self.mmio_read32(base + HDA_SD_CTL) & HDA_SD_CTL_SRST == 0 {
                    break;
                }
                core::hint::spin_loop();
            }

            // Format
            self.mmio_write16(base + HDA_SD_FMT, format.to_hda_format_register());

            // Cyclic Buffer Length (total bytes across all BDL entries)
            self.mmio_write32(base + HDA_SD_CBL, total_bytes);

            // Last Valid Index
            self.mmio_write16(base + HDA_SD_LVI, (BDL_ENTRIES as u16) - 1);

            // BDL base address
            self.mmio_write32(base + HDA_SD_BDLPL, bdl_phys as u32);
            self.mmio_write32(base + HDA_SD_BDLPU, (bdl_phys >> 32) as u32);

            // Stream tag [23:20] + enable IOC interrupt + descriptor error interrupt
            let stream_tag = ((id as u32) + 1) << 20;
            self.mmio_write32(
                base + HDA_SD_CTL,
                stream_tag | HDA_SD_CTL_IOCE | HDA_SD_CTL_DEIE,
            );
        }

        self.streams.push(stream);
        Ok(id)
    }

    pub fn start_stream(&mut self, stream_id: u8) -> Result<(), &'static str> {
        let idx = stream_id as usize;
        match self.streams.get(idx) {
            None => return Err("invalid stream id"),
            Some(s) if s.bdl.is_empty() => return Err("no buffers in BDL"),
            _ => {}
        }

        let base = self.stream_reg_base(stream_id);
        unsafe {
            let ctl = self.mmio_read32(base + HDA_SD_CTL);
            self.mmio_write32(base + HDA_SD_CTL, ctl | HDA_SD_CTL_RUN);
        }
        self.streams[idx].running = true;
        Ok(())
    }

    pub fn stop_stream(&mut self, stream_id: u8) -> Result<(), &'static str> {
        let idx = stream_id as usize;
        if idx >= self.streams.len() {
            return Err("invalid stream id");
        }

        let base = self.stream_reg_base(stream_id);
        unsafe {
            let ctl = self.mmio_read32(base + HDA_SD_CTL);
            self.mmio_write32(base + HDA_SD_CTL, ctl & !HDA_SD_CTL_RUN);
        }
        self.streams[idx].running = false;
        Ok(())
    }

    /// Read the Link Position In Buffer register for a running stream.
    pub fn stream_position(&self, stream_id: u8) -> Option<u32> {
        if (stream_id as usize) >= self.streams.len() {
            return None;
        }
        let base = self.stream_reg_base(stream_id);
        Some(unsafe { self.mmio_read32(base + HDA_SD_LPIB) })
    }
}

// ─── Audio Ring Buffer (lock-free SPSC) ─────────────────────────────────────
//
// Single-producer / single-consumer ring for f32 audio samples.
// The mixer (producer) pushes mixed samples; the DMA feeder thread
// (consumer) drains them into the HDA BDL buffer.  Atomic indices
// with Acquire/Release ordering provide synchronisation without locks.

pub struct AudioRingBuffer {
    buffer: *mut f32,
    size: usize,
    write_pos: AtomicUsize,
    read_pos: AtomicUsize,
}

// Safety: the raw pointer is heap-allocated and exclusively owned by
// this struct. The AtomicUsize indices synchronise all cross-thread access.
unsafe impl Send for AudioRingBuffer {}
unsafe impl Sync for AudioRingBuffer {}

impl AudioRingBuffer {
    pub fn new(size: usize) -> Self {
        // A zero size would make alloc_zeroed UB and every `% self.size` a
        // divide-by-zero panic on the audio mix path. Floor at one slot.
        let size = size.max(1);
        let layout = alloc::alloc::Layout::from_size_align(
            size * core::mem::size_of::<f32>(),
            core::mem::align_of::<f32>(),
        )
        .expect("invalid ring buffer layout");
        let buffer = unsafe { alloc::alloc::alloc_zeroed(layout) as *mut f32 };
        Self {
            buffer,
            size,
            write_pos: AtomicUsize::new(0),
            read_pos: AtomicUsize::new(0),
        }
    }

    /// Push samples into the ring. Returns the number actually written.
    pub fn write(&self, samples: &[f32]) -> usize {
        let w = self.write_pos.load(Ordering::Relaxed);
        let r = self.read_pos.load(Ordering::Acquire);

        let free = if w >= r {
            self.size - 1 - (w - r)
        } else {
            r - w - 1
        };
        let count = samples.len().min(free);

        for i in 0..count {
            let idx = (w + i) % self.size;
            unsafe {
                core::ptr::write(self.buffer.add(idx), samples[i]);
            }
        }
        self.write_pos
            .store((w + count) % self.size, Ordering::Release);
        count
    }

    /// Drain samples from the ring into `dest`. Returns the number read.
    pub fn read(&self, dest: &mut [f32]) -> usize {
        let r = self.read_pos.load(Ordering::Relaxed);
        let w = self.write_pos.load(Ordering::Acquire);

        let avail = if w >= r { w - r } else { self.size - r + w };
        let count = dest.len().min(avail);

        for i in 0..count {
            let idx = (r + i) % self.size;
            dest[i] = unsafe { core::ptr::read(self.buffer.add(idx)) };
        }
        self.read_pos
            .store((r + count) % self.size, Ordering::Release);
        count
    }

    pub fn available_read(&self) -> usize {
        let w = self.write_pos.load(Ordering::Acquire);
        let r = self.read_pos.load(Ordering::Relaxed);
        if w >= r {
            w - r
        } else {
            self.size - r + w
        }
    }

    pub fn available_write(&self) -> usize {
        let w = self.write_pos.load(Ordering::Relaxed);
        let r = self.read_pos.load(Ordering::Acquire);
        if w >= r {
            self.size - 1 - (w - r)
        } else {
            r - w - 1
        }
    }
}

// ─── Audio Mixer ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelType {
    Master,
    Application,
    System,
    Communication,
    Game,
    Music,
    Notification,
}

#[derive(Debug, Clone)]
pub struct MixerChannel {
    pub name: String,
    pub channel_type: ChannelType,
    pub volume: f32,
    pub muted: bool,
    pub peak_level: f32,
    pub rms_level: f32,
}

impl MixerChannel {
    pub fn new(name: &str, channel_type: ChannelType) -> Self {
        Self {
            name: String::from(name),
            channel_type,
            volume: 1.0,
            muted: false,
            peak_level: 0.0,
            rms_level: 0.0,
        }
    }

    pub fn effective_volume(&self) -> f32 {
        if self.muted {
            0.0
        } else {
            self.volume
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppAudioState {
    pub pid: u64,
    pub name: String,
    pub volume: f32,
    pub muted: bool,
    pub balance: f32,
    pub output_device: u64,
}

impl AppAudioState {
    pub fn new(pid: u64, name: &str) -> Self {
        Self {
            pid,
            name: String::from(name),
            volume: 1.0,
            muted: false,
            balance: 0.0,
            output_device: 0,
        }
    }

    pub fn left_gain(&self) -> f32 {
        let vol = if self.muted { 0.0 } else { self.volume };
        if self.balance < 0.0 {
            vol
        } else {
            vol * (1.0 - self.balance)
        }
    }

    pub fn right_gain(&self) -> f32 {
        let vol = if self.muted { 0.0 } else { self.volume };
        if self.balance > 0.0 {
            vol
        } else {
            vol * (1.0 + self.balance)
        }
    }
}

/// A mixer voice — a single audio stream the mixer reads and sums into
/// `mix_buffer` each period. This is the *real* producer feeding the mix;
/// the boot test-tone is registered as one of these (kind `Tone`) so it
/// exercises the mixer→ring path rather than poking the ring directly.
///
/// Generation is allocation-free on the hot path: each source owns a
/// persistent `scratch` buffer it regenerates into every period.
#[derive(Debug, Clone)]
pub struct MixerSource {
    pub id: u64,
    pub channel_type: ChannelType,
    pub gain: f32,
    pub active: bool,
    pub kind: SourceKind,
    /// Persistent per-source scratch — sized once, refilled every period.
    scratch: Vec<f32>,
}

#[derive(Debug, Clone)]
pub enum SourceKind {
    /// Square-wave test tone: (period_in_frames, phase_in_frames, amplitude).
    Tone {
        period_frames: usize,
        phase: usize,
        amplitude: f32,
    },
    /// App-fed PCM (interleaved i16-stereo converted to f32 on enqueue). A
    /// bounded SPSC-style queue the syscall path (`submit_samples`) fills and
    /// `generate()` drains. `pid` is the owning task so its source can be
    /// reclaimed on exit. No per-period heap alloc: `queue` is a persistent
    /// ring sized once at `PCM_QUEUE_SAMPLES`; underrun (queue empty) zero-fills.
    Pcm {
        /// Owning task id (per-app voice).
        pid: u64,
        /// Persistent ring buffer of interleaved-stereo f32 samples.
        queue: Vec<f32>,
        /// Producer (syscall) write cursor into `queue`.
        head: usize,
        /// Consumer (mixer) read cursor into `queue`.
        tail: usize,
    },
}

/// Per-PCM-source queue depth in f32 samples: room for `AUDIO_SUBMIT_MAX_FRAMES`
/// worth of stereo plus one period of slack so a full-size submit always fits
/// when the mixer has drained a period. Sized once; never reallocated.
const PCM_QUEUE_SAMPLES: usize = 512 * AUDIO_CHANNELS + AUDIO_PERIOD_SAMPLES;
/// Cap on simultaneously-registered app PCM voices (one per task). Bounds the
/// mixer's per-period work; further `submit_samples` from new pids are rejected.
const MAX_PCM_SOURCES: usize = 16;

impl MixerSource {
    fn new_tone(
        id: u64,
        channel_type: ChannelType,
        sample_rate: u32,
        hz: u32,
        amplitude: f32,
        capacity_samples: usize,
    ) -> Self {
        let period_frames = (sample_rate / hz.max(1)).max(2) as usize;
        Self {
            id,
            channel_type,
            gain: 1.0,
            active: true,
            kind: SourceKind::Tone {
                period_frames,
                phase: 0,
                amplitude,
            },
            scratch: alloc::vec![0.0; capacity_samples],
        }
    }

    /// Construct an app-fed PCM voice for `pid`. The queue is sized once
    /// (`PCM_QUEUE_SAMPLES`) and never reallocated; `scratch` matches the mix
    /// buffer so `generate()` never allocates.
    fn new_pcm(id: u64, pid: u64, channel_type: ChannelType, capacity_samples: usize) -> Self {
        Self {
            id,
            channel_type,
            gain: 1.0,
            active: true,
            kind: SourceKind::Pcm {
                pid,
                queue: alloc::vec![0.0; PCM_QUEUE_SAMPLES],
                head: 0,
                tail: 0,
            },
            scratch: alloc::vec![0.0; capacity_samples],
        }
    }

    /// Number of f32 samples currently queued in a PCM source (0 for others).
    fn pcm_queued(&self) -> usize {
        match &self.kind {
            SourceKind::Pcm {
                queue, head, tail, ..
            } => {
                let cap = queue.len();
                if *head >= *tail {
                    *head - *tail
                } else {
                    cap - *tail + *head
                }
            }
            _ => 0,
        }
    }

    /// Enqueue interleaved-stereo f32 `samples` into a PCM source's ring.
    /// Returns the number of f32 samples accepted (may be < `samples.len()`
    /// when the queue is near full — the caller resubmits the remainder). No
    /// allocation: writes into the persistent `queue`.
    fn pcm_enqueue(&mut self, samples: &[f32]) -> usize {
        if let SourceKind::Pcm {
            queue, head, tail, ..
        } = &mut self.kind
        {
            let cap = queue.len();
            // Free slots: leave one empty so head==tail unambiguously means empty.
            let used = if *head >= *tail {
                *head - *tail
            } else {
                cap - *tail + *head
            };
            let free = cap - 1 - used;
            let n = samples.len().min(free);
            for &s in &samples[..n] {
                queue[*head] = s;
                *head = (*head + 1) % cap;
            }
            n
        } else {
            0
        }
    }

    /// Generate `frames` stereo frames into the source's scratch buffer and
    /// return the filled interleaved slice. No allocation on the hot path —
    /// `scratch` was sized at construction.
    fn generate(&mut self, frames: usize) -> &[f32] {
        let samples = (frames * AUDIO_CHANNELS).min(self.scratch.len());
        match &mut self.kind {
            SourceKind::Tone {
                period_frames,
                phase,
                amplitude,
            } => {
                let half = (*period_frames / 2).max(1);
                let amp = *amplitude;
                let mut i = 0;
                while i + 1 < samples {
                    let v = if *phase < half { amp } else { -amp };
                    self.scratch[i] = v;
                    self.scratch[i + 1] = v;
                    *phase += 1;
                    if *phase >= *period_frames {
                        *phase = 0;
                    }
                    i += 2;
                }
            }
            SourceKind::Pcm {
                queue, head, tail, ..
            } => {
                // Drain up to `samples` from the ring; zero-fill the remainder
                // (underrun is the normal "app produced no data this period"
                // case — silence, never a glitch). No allocation.
                let cap = queue.len();
                let mut i = 0;
                while i < samples {
                    if *tail != *head {
                        self.scratch[i] = queue[*tail];
                        *tail = (*tail + 1) % cap;
                    } else {
                        self.scratch[i] = 0.0;
                    }
                    i += 1;
                }
            }
        }
        &self.scratch[..samples]
    }
}

pub struct AudioMixer {
    pub master_volume: f32,
    pub per_app_volumes: BTreeMap<u64, AppAudioState>,
    pub channels: Vec<MixerChannel>,
    pub output_format: AudioStreamFormat,
    pub mix_buffer: Vec<f32>,
    pub spatial_enabled: bool,
    /// Registered voices the mixer sums into `mix_buffer` each period.
    pub sources: Vec<MixerSource>,
    /// Frames of audio produced into `mix_buffer` by the last `mix()`.
    pub last_mixed_frames: usize,
}

impl AudioMixer {
    pub fn new(format: AudioStreamFormat) -> Self {
        // Mix one DMA period (128 frames) per tick so the mixer cadence
        // matches the SCHED_BODY audio thread's drain period exactly.
        let buffer_samples = AUDIO_PERIOD_SAMPLES;
        Self {
            master_volume: 0.8,
            per_app_volumes: BTreeMap::new(),
            channels: alloc::vec![
                MixerChannel::new("Master", ChannelType::Master),
                MixerChannel::new("Applications", ChannelType::Application),
                MixerChannel::new("System", ChannelType::System),
                MixerChannel::new("Communication", ChannelType::Communication),
                MixerChannel::new("Game", ChannelType::Game),
                MixerChannel::new("Music", ChannelType::Music),
                MixerChannel::new("Notifications", ChannelType::Notification),
            ],
            output_format: format,
            mix_buffer: alloc::vec![0.0; buffer_samples],
            spatial_enabled: false,
            sources: Vec::new(),
            last_mixed_frames: 0,
        }
    }

    /// Register a square-wave test tone as a real mixer voice. Returns the
    /// source id. The tone is then summed into `mix_buffer` by `mix()` and
    /// flows through `process_tick` into AUDIO_RING — the production path,
    /// not the direct-ring-write shortcut.
    pub fn register_test_tone(&mut self, sample_rate: u32, hz: u32) -> u64 {
        let id = self.next_source_id();
        let src = MixerSource::new_tone(
            id,
            ChannelType::System,
            sample_rate,
            hz,
            0.18,
            self.mix_buffer.len(),
        );
        self.sources.push(src);
        id
    }

    pub fn remove_source(&mut self, id: u64) {
        self.sources.retain(|s| s.id != id);
    }

    /// Enqueue app-fed interleaved-stereo f32 PCM for `pid`, creating the
    /// task's `SourceKind::Pcm` voice on first submit. Returns the number of
    /// f32 samples accepted, or `None` if the per-app voice cap is hit (a brand
    /// new pid when `MAX_PCM_SOURCES` PCM voices already exist). No allocation
    /// on the steady-state path: the queue is sized once at creation.
    pub fn submit_pcm(&mut self, pid: u64, samples: &[f32]) -> Option<usize> {
        // Find the caller's existing PCM voice.
        if let Some(src) = self
            .sources
            .iter_mut()
            .find(|s| matches!(&s.kind, SourceKind::Pcm { pid: p, .. } if *p == pid))
        {
            return Some(src.pcm_enqueue(samples));
        }
        // No voice yet — create one if we're under the cap.
        let pcm_count = self
            .sources
            .iter()
            .filter(|s| matches!(s.kind, SourceKind::Pcm { .. }))
            .count();
        if pcm_count >= MAX_PCM_SOURCES {
            return None;
        }
        let id = self.next_source_id();
        let mut src =
            MixerSource::new_pcm(id, pid, ChannelType::Application, self.mix_buffer.len());
        let n = src.pcm_enqueue(samples);
        self.sources.push(src);
        Some(n)
    }

    /// Remove every PCM voice owned by `pid` (called from task-exit reclaim so
    /// a dead app's voice stops streaming silence forever). Tone voices are
    /// untouched. Returns the number of sources removed.
    pub fn remove_task_sources(&mut self, pid: u64) -> usize {
        let before = self.sources.len();
        self.sources
            .retain(|s| !matches!(&s.kind, SourceKind::Pcm { pid: p, .. } if *p == pid));
        before - self.sources.len()
    }

    /// Total f32 samples queued across this PID's PCM voice (0 if none). Used by
    /// the submit smoketest to prove enqueue landed in the source's ring.
    pub fn pcm_queued_for(&self, pid: u64) -> usize {
        self.sources
            .iter()
            .filter(|s| matches!(&s.kind, SourceKind::Pcm { pid: p, .. } if *p == pid))
            .map(|s| s.pcm_queued())
            .sum()
    }

    pub fn active_source_count(&self) -> usize {
        self.sources.iter().filter(|s| s.active).count()
    }

    fn next_source_id(&self) -> u64 {
        self.sources
            .iter()
            .map(|s| s.id)
            .max()
            .map(|m| m + 1)
            .unwrap_or(1)
    }

    pub fn register_app(&mut self, pid: u64, name: &str) {
        self.per_app_volumes
            .insert(pid, AppAudioState::new(pid, name));
    }

    pub fn unregister_app(&mut self, pid: u64) {
        self.per_app_volumes.remove(&pid);
    }

    pub fn set_app_volume(&mut self, pid: u64, volume: f32) {
        if let Some(state) = self.per_app_volumes.get_mut(&pid) {
            state.volume = volume.clamp(0.0, 1.0);
        }
    }

    pub fn set_app_mute(&mut self, pid: u64, muted: bool) {
        if let Some(state) = self.per_app_volumes.get_mut(&pid) {
            state.muted = muted;
        }
    }

    pub fn set_app_balance(&mut self, pid: u64, balance: f32) {
        if let Some(state) = self.per_app_volumes.get_mut(&pid) {
            state.balance = balance.clamp(-1.0, 1.0);
        }
    }

    pub fn set_master_volume(&mut self, volume: f32) {
        self.master_volume = volume.clamp(0.0, 1.0);
    }

    pub fn set_channel_volume(&mut self, channel_type: ChannelType, volume: f32) {
        for ch in &mut self.channels {
            if ch.channel_type == channel_type {
                ch.volume = volume.clamp(0.0, 1.0);
            }
        }
    }

    pub fn set_channel_mute(&mut self, channel_type: ChannelType, muted: bool) {
        for ch in &mut self.channels {
            if ch.channel_type == channel_type {
                ch.muted = muted;
            }
        }
    }

    /// Sum all active voices into `mix_buffer`, apply master + channel gain,
    /// and clamp. This is the PRODUCER half of the audio pipeline: it fills
    /// `mix_buffer`, which `AudioManager::process_tick` then writes into
    /// AUDIO_RING. It does NOT read the ring (the drain thread is the sole
    /// ring consumer — SPSC discipline).
    pub fn mix(&mut self) {
        // Zero the accumulator for this period.
        for s in self.mix_buffer.iter_mut() {
            *s = 0.0;
        }

        let frames = self.mix_buffer.len() / AUDIO_CHANNELS;
        let len = self.mix_buffer.len();

        // Additively sum each active voice. `generate` writes into the
        // source's own persistent scratch — no per-period heap allocation.
        for src in self.sources.iter_mut() {
            if !src.active {
                continue;
            }
            let gain = src.gain;
            let s = src.generate(frames);
            let n = s.len().min(len);
            for i in 0..n {
                self.mix_buffer[i] += s[i] * gain;
            }
        }

        let master = self.master_volume;
        let master_ch = self
            .channels
            .iter()
            .find(|c| c.channel_type == ChannelType::Master)
            .map(|c| c.effective_volume())
            .unwrap_or(1.0);

        let final_gain = master * master_ch;

        for sample in self.mix_buffer.iter_mut() {
            *sample = (*sample * final_gain).clamp(-1.0, 1.0);
        }

        // Frames the mixer produced this period: full buffer when any voice
        // is active, zero (silence) otherwise. Used by the smoketest to
        // prove the producer ran.
        self.last_mixed_frames = if self.active_source_count() > 0 {
            frames
        } else {
            0
        };
    }

    /// Additive mix of N input streams with master-volume + channel gain.
    /// Each input slice contains interleaved stereo f32 samples.
    pub fn mix_streams(&mut self, inputs: &[&[f32]]) {
        for s in self.mix_buffer.iter_mut() {
            *s = 0.0;
        }

        let len = self.mix_buffer.len();
        for input in inputs {
            let n = (*input).len().min(len);
            for i in 0..n {
                self.mix_buffer[i] += input[i];
            }
        }

        let master = self.master_volume;
        let master_ch = self
            .channels
            .iter()
            .find(|c| c.channel_type == ChannelType::Master)
            .map(|c| c.effective_volume())
            .unwrap_or(1.0);
        let gain = master * master_ch;

        for s in self.mix_buffer.iter_mut() {
            *s = (*s * gain).clamp(-1.0, 1.0);
        }
    }

    pub fn clear_buffer(&mut self) {
        for sample in self.mix_buffer.iter_mut() {
            *sample = 0.0;
        }
    }

    pub fn update_levels(&mut self) {
        for ch in &mut self.channels {
            ch.peak_level *= 0.95;
            ch.rms_level *= 0.95;
        }
    }
}

/// Convert f32 samples (−1.0 … +1.0) to signed 16-bit PCM for the HDA DMA buffer.
pub fn f32_to_i16(input: &[f32], output: &mut [i16]) {
    let n = input.len().min(output.len());
    for i in 0..n {
        let clamped = input[i].clamp(-1.0, 1.0);
        output[i] = (clamped * 32767.0) as i16;
    }
}

// ─── Audio Routing Engine ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointType {
    Speaker,
    Headphone,
    Hdmi,
    Spdif,
    Microphone,
    LineIn,
    UsbAudio,
    Bluetooth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointState {
    Active,
    Idle,
    Suspended,
    Unavailable,
}

#[derive(Debug, Clone)]
pub struct AudioEndpoint {
    pub id: u64,
    pub name: String,
    pub endpoint_type: EndpointType,
    pub format: AudioStreamFormat,
    pub latency_us: u32,
    pub state: EndpointState,
}

impl AudioEndpoint {
    pub fn new(id: u64, name: &str, ep_type: EndpointType, format: AudioStreamFormat) -> Self {
        Self {
            id,
            name: String::from(name),
            endpoint_type: ep_type,
            format,
            latency_us: 0,
            state: EndpointState::Idle,
        }
    }

    pub fn is_output(&self) -> bool {
        matches!(
            self.endpoint_type,
            EndpointType::Speaker
                | EndpointType::Headphone
                | EndpointType::Hdmi
                | EndpointType::Spdif
                | EndpointType::UsbAudio
                | EndpointType::Bluetooth
        )
    }

    pub fn is_input(&self) -> bool {
        matches!(
            self.endpoint_type,
            EndpointType::Microphone | EndpointType::LineIn
        )
    }

    pub fn activate(&mut self) {
        self.state = EndpointState::Active;
    }

    pub fn suspend(&mut self) {
        self.state = EndpointState::Suspended;
    }
}

#[derive(Debug, Clone)]
pub struct VirtualDevice {
    pub id: u64,
    pub name: String,
    pub channels: u8,
    pub sample_rate: u32,
    pub buffer: Vec<f32>,
}

impl VirtualDevice {
    pub fn new(id: u64, name: &str, channels: u8, sample_rate: u32) -> Self {
        let buf_size = (sample_rate / 100) as usize * channels as usize;
        Self {
            id,
            name: String::from(name),
            channels,
            sample_rate,
            buffer: alloc::vec![0.0; buf_size],
        }
    }
}

// ─── Audio Effects ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct EqBand {
    pub frequency: f32,
    pub gain_db: f32,
    pub q: f32,
}

#[derive(Debug, Clone)]
pub struct CompressorParams {
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub makeup_gain_db: f32,
}

impl CompressorParams {
    pub fn default_mastering() -> Self {
        Self {
            threshold_db: -6.0,
            ratio: 4.0,
            attack_ms: 5.0,
            release_ms: 50.0,
            makeup_gain_db: 2.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReverbParams {
    pub room_size: f32,
    pub damping: f32,
    pub wet: f32,
    pub dry: f32,
    pub width: f32,
}

impl ReverbParams {
    pub fn small_room() -> Self {
        Self {
            room_size: 0.3,
            damping: 0.5,
            wet: 0.2,
            dry: 0.8,
            width: 0.8,
        }
    }

    pub fn large_hall() -> Self {
        Self {
            room_size: 0.9,
            damping: 0.3,
            wet: 0.4,
            dry: 0.6,
            width: 1.0,
        }
    }
}

#[derive(Debug, Clone)]
pub enum AudioEffect {
    Equalizer(Vec<EqBand>),
    Compressor(CompressorParams),
    NoiseGate(f32),
    Reverb(ReverbParams),
    SpatialPosition(f32, f32, f32),
}

#[derive(Debug, Clone)]
pub struct AudioRoute {
    pub source: u64,
    pub destination: u64,
    pub gain: f32,
    pub muted: bool,
    pub effects: Vec<AudioEffect>,
}

impl AudioRoute {
    pub fn new(source: u64, destination: u64) -> Self {
        Self {
            source,
            destination,
            gain: 1.0,
            muted: false,
            effects: Vec::new(),
        }
    }

    pub fn with_gain(mut self, gain: f32) -> Self {
        self.gain = gain;
        self
    }

    pub fn add_effect(&mut self, effect: AudioEffect) {
        self.effects.push(effect);
    }

    pub fn effective_gain(&self) -> f32 {
        if self.muted {
            0.0
        } else {
            self.gain
        }
    }
}

pub struct AudioRouter {
    pub physical_inputs: Vec<AudioEndpoint>,
    pub physical_outputs: Vec<AudioEndpoint>,
    pub virtual_inputs: Vec<VirtualDevice>,
    pub virtual_outputs: Vec<VirtualDevice>,
    pub routes: Vec<AudioRoute>,
    pub matrix: Vec<Vec<f32>>,
    next_endpoint_id: u64,
    next_virtual_id: u64,
}

impl AudioRouter {
    pub fn new() -> Self {
        Self {
            physical_inputs: Vec::new(),
            physical_outputs: Vec::new(),
            virtual_inputs: Vec::new(),
            virtual_outputs: Vec::new(),
            routes: Vec::new(),
            matrix: Vec::new(),
            next_endpoint_id: 1,
            next_virtual_id: 0x1000,
        }
    }

    pub fn add_physical_output(
        &mut self,
        name: &str,
        ep_type: EndpointType,
        format: AudioStreamFormat,
    ) -> u64 {
        let id = self.next_endpoint_id;
        self.next_endpoint_id += 1;
        self.physical_outputs
            .push(AudioEndpoint::new(id, name, ep_type, format));
        self.rebuild_matrix();
        id
    }

    pub fn add_physical_input(
        &mut self,
        name: &str,
        ep_type: EndpointType,
        format: AudioStreamFormat,
    ) -> u64 {
        let id = self.next_endpoint_id;
        self.next_endpoint_id += 1;
        self.physical_inputs
            .push(AudioEndpoint::new(id, name, ep_type, format));
        self.rebuild_matrix();
        id
    }

    pub fn add_virtual_output(&mut self, name: &str, channels: u8, sample_rate: u32) -> u64 {
        let id = self.next_virtual_id;
        self.next_virtual_id += 1;
        self.virtual_outputs
            .push(VirtualDevice::new(id, name, channels, sample_rate));
        self.rebuild_matrix();
        id
    }

    pub fn add_virtual_input(&mut self, name: &str, channels: u8, sample_rate: u32) -> u64 {
        let id = self.next_virtual_id;
        self.next_virtual_id += 1;
        self.virtual_inputs
            .push(VirtualDevice::new(id, name, channels, sample_rate));
        self.rebuild_matrix();
        id
    }

    pub fn add_route(&mut self, source: u64, destination: u64) -> usize {
        let idx = self.routes.len();
        self.routes.push(AudioRoute::new(source, destination));
        self.update_matrix_from_routes();
        idx
    }

    pub fn remove_route(&mut self, index: usize) {
        if index < self.routes.len() {
            self.routes.remove(index);
            self.update_matrix_from_routes();
        }
    }

    pub fn set_route_gain(&mut self, index: usize, gain: f32) {
        if let Some(route) = self.routes.get_mut(index) {
            route.gain = gain.clamp(0.0, 4.0);
            self.update_matrix_from_routes();
        }
    }

    pub fn set_route_mute(&mut self, index: usize, muted: bool) {
        if let Some(route) = self.routes.get_mut(index) {
            route.muted = muted;
            self.update_matrix_from_routes();
        }
    }

    fn total_inputs(&self) -> usize {
        self.physical_inputs.len() + self.virtual_inputs.len()
    }

    fn total_outputs(&self) -> usize {
        self.physical_outputs.len() + self.virtual_outputs.len()
    }

    fn rebuild_matrix(&mut self) {
        let inputs = self.total_inputs();
        let outputs = self.total_outputs();
        self.matrix = alloc::vec![alloc::vec![0.0; outputs]; inputs];
    }

    fn input_index(&self, id: u64) -> Option<usize> {
        if let Some(pos) = self.physical_inputs.iter().position(|e| e.id == id) {
            return Some(pos);
        }
        if let Some(pos) = self.virtual_inputs.iter().position(|v| v.id == id) {
            return Some(self.physical_inputs.len() + pos);
        }
        None
    }

    fn output_index(&self, id: u64) -> Option<usize> {
        if let Some(pos) = self.physical_outputs.iter().position(|e| e.id == id) {
            return Some(pos);
        }
        if let Some(pos) = self.virtual_outputs.iter().position(|v| v.id == id) {
            return Some(self.physical_outputs.len() + pos);
        }
        None
    }

    fn update_matrix_from_routes(&mut self) {
        self.rebuild_matrix();
        for route in &self.routes {
            if let (Some(src_idx), Some(dst_idx)) = (
                self.input_index(route.source),
                self.output_index(route.destination),
            ) {
                if src_idx < self.matrix.len() && dst_idx < self.matrix[src_idx].len() {
                    self.matrix[src_idx][dst_idx] = route.effective_gain();
                }
            }
        }
    }

    pub fn get_matrix_gain(&self, input: usize, output: usize) -> f32 {
        self.matrix
            .get(input)
            .and_then(|row| row.get(output))
            .copied()
            .unwrap_or(0.0)
    }
}

// ─── Audio Capture ──────────────────────────────────────────────────────────

pub static CAPTURE_RING: spin::Once<AudioRingBuffer> = spin::Once::new();

/// Loopback capture: taps the output mix so apps can record system audio
/// without a hardware input device.
pub struct LoopbackCapture {
    pub enabled: bool,
    pub buffer: Vec<f32>,
    pub write_pos: usize,
    pub capacity: usize,
}

impl LoopbackCapture {
    pub fn new(capacity: usize) -> Self {
        Self {
            enabled: false,
            buffer: alloc::vec![0.0; capacity],
            write_pos: 0,
            capacity,
        }
    }

    pub fn enable(&mut self) {
        self.enabled = true;
    }
    pub fn disable(&mut self) {
        self.enabled = false;
    }

    /// Feed output-mix samples into the loopback ring.
    pub fn feed(&mut self, samples: &[f32]) {
        if !self.enabled {
            return;
        }
        for &s in samples {
            self.buffer[self.write_pos] = s;
            self.write_pos = (self.write_pos + 1) % self.capacity;
        }
    }

    /// Read captured loopback samples into `dest`. Returns count read.
    pub fn read(&mut self, dest: &mut [f32], read_pos: &mut usize) -> usize {
        if !self.enabled {
            return 0;
        }
        let mut count = 0;
        while *read_pos != self.write_pos && count < dest.len() {
            dest[count] = self.buffer[*read_pos];
            *read_pos = (*read_pos + 1) % self.capacity;
            count += 1;
        }
        count
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureState {
    Stopped,
    Running,
    Paused,
}

pub struct CaptureSession {
    pub state: CaptureState,
    pub format: AudioStreamFormat,
    pub stream_id: Option<u8>,
    pub frames_captured: u64,
    pub loopback_read_pos: usize,
    pub use_loopback: bool,
}

impl CaptureSession {
    pub fn new(format: AudioStreamFormat) -> Self {
        Self {
            state: CaptureState::Stopped,
            format,
            stream_id: None,
            frames_captured: 0,
            loopback_read_pos: 0,
            use_loopback: false,
        }
    }

    pub fn is_running(&self) -> bool {
        self.state == CaptureState::Running
    }
}

impl HdaController {
    /// Set up an HDA input stream for capture (mirrors output stream setup).
    pub fn setup_capture_stream(&mut self, format: AudioStreamFormat) -> Result<u8, &'static str> {
        use x86_64::structures::paging::FrameAllocator;
        let mut alloc = crate::memory::GlobalFrameAllocator;

        let stream_index = self.num_output_streams
            + (self
                .streams
                .iter()
                .filter(|s| s.direction == StreamDirection::Capture)
                .count() as u8);
        let id = self.streams.len() as u8;
        let mut stream = HdaStream::new(id, StreamDirection::Capture, format);

        let bytes_per_period = (AUDIO_PERIOD_FRAMES
            * format.channels as usize
            * (format.bits_per_sample as usize / 8)) as u32;

        let bdl_frame = alloc
            .allocate_frame()
            .ok_or("failed to allocate capture BDL page")?;
        let bdl_phys = bdl_frame.start_address().as_u64();
        let bdl_virt = phys_to_virt(bdl_phys);
        unsafe {
            core::ptr::write_bytes(bdl_virt as *mut u8, 0, 4096);
        }

        let dma_frame = alloc
            .allocate_frame()
            .ok_or("failed to allocate capture DMA page")?;
        let dma_phys = dma_frame.start_address().as_u64();
        let dma_virt = phys_to_virt(dma_phys);
        unsafe {
            core::ptr::write_bytes(dma_virt as *mut u8, 0, 4096);
        }

        let total_bytes = bytes_per_period * BDL_ENTRIES as u32;
        stream.bdl_phys = bdl_phys;
        stream.dma_phys = dma_phys;
        stream.dma_virt = dma_virt;
        stream.dma_size = total_bytes;

        for i in 0..BDL_ENTRIES {
            let buf_offset = (i as u32) * bytes_per_period;
            let entry_addr = bdl_virt + (i as u64) * 16;
            let ioc = i == BDL_ENTRIES - 1;
            let entry = HdaBdlEntry {
                address: dma_phys + buf_offset as u64,
                length: bytes_per_period,
                flags: if ioc { 1 } else { 0 },
            };
            unsafe {
                core::ptr::write_volatile(entry_addr as *mut HdaBdlEntry, entry);
            }
            stream.add_buffer(dma_phys + buf_offset as u64, bytes_per_period, ioc);
        }

        let base = self.stream_reg_base(stream_index);
        unsafe {
            let ctl = self.mmio_read32(base + HDA_SD_CTL) & 0x00FF_FFFF;
            self.mmio_write32(base + HDA_SD_CTL, ctl | HDA_SD_CTL_SRST);
            for _ in 0..1_000 {
                if self.mmio_read32(base + HDA_SD_CTL) & HDA_SD_CTL_SRST != 0 {
                    break;
                }
                core::hint::spin_loop();
            }
            self.mmio_write32(base + HDA_SD_CTL, ctl & !HDA_SD_CTL_SRST);
            for _ in 0..1_000 {
                if self.mmio_read32(base + HDA_SD_CTL) & HDA_SD_CTL_SRST == 0 {
                    break;
                }
                core::hint::spin_loop();
            }

            self.mmio_write16(base + HDA_SD_FMT, format.to_hda_format_register());
            self.mmio_write32(base + HDA_SD_CBL, total_bytes);
            self.mmio_write16(base + HDA_SD_LVI, (BDL_ENTRIES as u16) - 1);
            self.mmio_write32(base + HDA_SD_BDLPL, bdl_phys as u32);
            self.mmio_write32(base + HDA_SD_BDLPU, (bdl_phys >> 32) as u32);

            let stream_tag = ((stream_index as u32) + 1) << 20;
            self.mmio_write32(
                base + HDA_SD_CTL,
                stream_tag | HDA_SD_CTL_IOCE | HDA_SD_CTL_DEIE,
            );
        }

        self.streams.push(stream);
        Ok(id)
    }

    /// Read captured i16 PCM from the capture DMA buffer, convert to f32.
    pub fn read_capture_samples(
        &self,
        stream_id: u8,
        dest: &mut [f32],
        read_offset: &mut u32,
    ) -> usize {
        let idx = stream_id as usize;
        let stream = match self.streams.get(idx) {
            Some(s) if s.direction == StreamDirection::Capture && s.running => s,
            _ => return 0,
        };

        let base = self.stream_reg_base(stream_id);
        let lpib = unsafe { self.mmio_read32(base + HDA_SD_LPIB) };

        let avail_bytes = if lpib >= *read_offset {
            lpib - *read_offset
        } else {
            stream.dma_size - *read_offset + lpib
        };

        let sample_count = (avail_bytes / 2) as usize;
        let count = sample_count.min(dest.len());

        for i in 0..count {
            let byte_off = (*read_offset + (i as u32) * 2) % stream.dma_size;
            let ptr = (stream.dma_virt + byte_off as u64) as *const i16;
            let raw = unsafe { core::ptr::read_volatile(ptr) };
            dest[i] = raw as f32 / 32767.0;
        }

        *read_offset = (*read_offset + (count as u32) * 2) % stream.dma_size;
        count
    }
}

impl AudioManager {
    pub fn start_capture(
        &mut self,
        format: AudioStreamFormat,
        loopback: bool,
    ) -> Result<(), &'static str> {
        if loopback {
            let mut session = CaptureSession::new(format);
            session.use_loopback = true;
            session.state = CaptureState::Running;
            self.capture_session = Some(session);
            return Ok(());
        }

        let hda = self.hda.as_mut().ok_or("no HDA controller")?;
        let cap_id = hda.setup_capture_stream(format)?;
        hda.start_stream(cap_id)?;
        let mut session = CaptureSession::new(format);
        session.stream_id = Some(cap_id);
        session.state = CaptureState::Running;
        self.capture_session = Some(session);
        Ok(())
    }

    pub fn stop_capture(&mut self) -> Result<(), &'static str> {
        if let Some(ref mut session) = self.capture_session {
            if let Some(sid) = session.stream_id {
                if let Some(ref mut hda) = self.hda {
                    hda.stop_stream(sid)?;
                }
            }
            session.state = CaptureState::Stopped;
        }
        self.capture_session = None;
        Ok(())
    }

    pub fn read_capture(&mut self, dest: &mut [f32]) -> usize {
        let loopback = &mut self.loopback;
        if let Some(ref mut session) = self.capture_session {
            if !session.is_running() {
                return 0;
            }

            if session.use_loopback {
                let count = loopback.read(dest, &mut session.loopback_read_pos);
                session.frames_captured += count as u64;
                return count;
            }

            if let Some(sid) = session.stream_id {
                if let Some(ref hda) = self.hda {
                    let mut read_off = (session.frames_captured as u32 * 2) % 4096;
                    let count = hda.read_capture_samples(sid, dest, &mut read_off);
                    session.frames_captured += count as u64;
                    return count;
                }
            }
        }
        0
    }
}

// ─── Spatial Audio ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct AudioListener3D {
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub up: [f32; 3],
}

impl AudioListener3D {
    pub fn default_position() -> Self {
        Self {
            position: [0.0, 0.0, 0.0],
            forward: [0.0, 0.0, -1.0],
            up: [0.0, 1.0, 0.0],
        }
    }

    pub fn right(&self) -> [f32; 3] {
        let fx = self.forward[0];
        let fy = self.forward[1];
        let fz = self.forward[2];
        let ux = self.up[0];
        let uy = self.up[1];
        let uz = self.up[2];
        [fy * uz - fz * uy, fz * ux - fx * uz, fx * uy - fy * ux]
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AudioSource3D {
    pub id: u64,
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub volume: f32,
    pub rolloff: f32,
    pub min_distance: f32,
    pub max_distance: f32,
    pub looping: bool,
}

impl AudioSource3D {
    pub fn new(id: u64, position: [f32; 3]) -> Self {
        Self {
            id,
            position,
            velocity: [0.0; 3],
            volume: 1.0,
            rolloff: 1.0,
            min_distance: 1.0,
            max_distance: 100.0,
            looping: false,
        }
    }

    pub fn distance_to(&self, listener: &AudioListener3D) -> f32 {
        let dx = self.position[0] - listener.position[0];
        let dy = self.position[1] - listener.position[1];
        let dz = self.position[2] - listener.position[2];
        sqrt_approx(dx * dx + dy * dy + dz * dz)
    }

    pub fn attenuation(&self, listener: &AudioListener3D) -> f32 {
        let dist = self.distance_to(listener);
        if dist <= self.min_distance {
            return self.volume;
        }
        if dist >= self.max_distance {
            return 0.0;
        }
        let att =
            self.min_distance / (self.min_distance + self.rolloff * (dist - self.min_distance));
        self.volume * att
    }

    /// Returns (left_gain, right_gain) based on source position relative to the listener.
    pub fn stereo_pan(&self, listener: &AudioListener3D) -> (f32, f32) {
        let dx = self.position[0] - listener.position[0];
        let dy = self.position[1] - listener.position[1];
        let dz = self.position[2] - listener.position[2];
        let dist = sqrt_approx(dx * dx + dy * dy + dz * dz);
        if dist < 0.001 {
            return (0.5, 0.5);
        }

        let right = listener.right();
        let dot = (dx * right[0] + dy * right[1] + dz * right[2]) / dist;
        let pan = dot.clamp(-1.0, 1.0);
        let left_gain = (1.0 - pan) * 0.5;
        let right_gain = (1.0 + pan) * 0.5;
        (left_gain, right_gain)
    }
}

/// Fast inverse-square-root-based sqrt approximation (no libm needed).
fn sqrt_approx(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let bits = x.to_bits();
    let approx = f32::from_bits((bits >> 1) + 0x1FC0_0000);
    0.5 * (approx + x / approx)
}

// ─── HRTF Spatial Audio Engine ──────────────────────────────────────────────
//
// Head-Related Transfer Function: per-source directional filtering that
// produces convincing 3D audio over headphones.  Uses pre-computed
// coefficients for 72 azimuth directions (5° resolution) at 0° elevation.
//
// Two binaural cues:
//   ITD — Interaural Time Delay: fractional-sample delay between ears
//   ILD — Interaural Level Difference: frequency-dependent gain difference
//
// The HRTF table is compact (~2.3 KiB) and runs in the SCHED_BODY audio
// thread, so every lookup must be O(1) with no allocations.

const HRTF_DIRECTIONS: usize = 72; // 360° / 5° = 72 entries
const HRTF_FILTER_TAPS: usize = 8; // short FIR per ear
const HRTF_HEAD_RADIUS: f32 = 0.0875; // average human head radius in metres
const SPEED_OF_SOUND: f32 = 343.0; // m/s at ~20 °C

#[derive(Clone, Copy)]
pub struct HrtfCoeffs {
    pub left_fir: [f32; HRTF_FILTER_TAPS],
    pub right_fir: [f32; HRTF_FILTER_TAPS],
    pub itd_samples_left: f32,
    pub itd_samples_right: f32,
    pub ild_left: f32,
    pub ild_right: f32,
}

/// Pre-computed HRTF table for 72 azimuth directions at 0° elevation.
/// Generated once at init time — no runtime allocations.
fn build_hrtf_table(sample_rate: u32) -> [HrtfCoeffs; HRTF_DIRECTIONS] {
    let mut table = [HrtfCoeffs {
        left_fir: [0.0; HRTF_FILTER_TAPS],
        right_fir: [0.0; HRTF_FILTER_TAPS],
        itd_samples_left: 0.0,
        itd_samples_right: 0.0,
        ild_left: 1.0,
        ild_right: 1.0,
    }; HRTF_DIRECTIONS];

    let sr = sample_rate as f32;

    for i in 0..HRTF_DIRECTIONS {
        let azimuth_deg = (i as f32) * 5.0;
        let az_rad = azimuth_deg * (core::f32::consts::PI / 180.0);

        // Woodworth ITD model: Δt = (r/c)(θ + sin θ) for the far ear
        let sin_az = sin_fast(az_rad);
        let itd_sec = (HRTF_HEAD_RADIUS / SPEED_OF_SOUND) * (az_rad + sin_az);
        let itd_samples = itd_sec * sr;

        // For azimuth 0-180°: source is on the right hemisphere → right ear leads
        let (left_delay, right_delay) = if azimuth_deg <= 180.0 {
            (itd_samples, 0.0)
        } else {
            (0.0, itd_samples)
        };

        // ILD: simple cosine model — far ear attenuated proportional to angle
        let cos_az = cos_fast(az_rad);
        let ild_near = 1.0;
        let ild_far = (0.5 + 0.5 * cos_az.abs()).max(0.2);
        let (ild_l, ild_r) = if azimuth_deg <= 180.0 {
            (ild_far, ild_near)
        } else {
            (ild_near, ild_far)
        };

        // Simplified FIR: low-pass for far ear (head shadow), all-pass for near ear.
        // Near ear: unit impulse at tap 0.
        // Far ear: 3-tap low-pass approximation of head shadow.
        let mut left_fir = [0.0f32; HRTF_FILTER_TAPS];
        let mut right_fir = [0.0f32; HRTF_FILTER_TAPS];

        if azimuth_deg <= 180.0 {
            // Right ear is near
            right_fir[0] = 1.0;
            left_fir[0] = 0.25;
            left_fir[1] = 0.5;
            left_fir[2] = 0.25;
        } else {
            left_fir[0] = 1.0;
            right_fir[0] = 0.25;
            right_fir[1] = 0.5;
            right_fir[2] = 0.25;
        }

        table[i] = HrtfCoeffs {
            left_fir,
            right_fir,
            itd_samples_left: left_delay,
            itd_samples_right: right_delay,
            ild_left: ild_l,
            ild_right: ild_r,
        };
    }

    table
}

/// Per-source HRTF state: delay lines + FIR history.
pub struct HrtfSourceState {
    pub source_id: u64,
    pub delay_line_left: [f32; 64],
    pub delay_line_right: [f32; 64],
    pub delay_write_pos: usize,
    pub fir_history_left: [f32; HRTF_FILTER_TAPS],
    pub fir_history_right: [f32; HRTF_FILTER_TAPS],
}

impl HrtfSourceState {
    pub fn new(source_id: u64) -> Self {
        Self {
            source_id,
            delay_line_left: [0.0; 64],
            delay_line_right: [0.0; 64],
            delay_write_pos: 0,
            fir_history_left: [0.0; HRTF_FILTER_TAPS],
            fir_history_right: [0.0; HRTF_FILTER_TAPS],
        }
    }
}

pub struct SpatialAudioEngine {
    pub listener: AudioListener3D,
    pub sources: Vec<AudioSource3D>,
    pub hrtf_enabled: bool,
    pub max_sources: usize,
    hrtf_table: [HrtfCoeffs; HRTF_DIRECTIONS],
    hrtf_states: Vec<HrtfSourceState>,
    sample_rate: u32,
}

impl SpatialAudioEngine {
    pub fn new(max_sources: usize) -> Self {
        Self {
            listener: AudioListener3D::default_position(),
            sources: Vec::new(),
            hrtf_enabled: false,
            max_sources,
            hrtf_table: build_hrtf_table(48000),
            hrtf_states: Vec::new(),
            sample_rate: 48000,
        }
    }

    pub fn new_with_sample_rate(max_sources: usize, sample_rate: u32) -> Self {
        Self {
            listener: AudioListener3D::default_position(),
            sources: Vec::new(),
            hrtf_enabled: false,
            max_sources,
            hrtf_table: build_hrtf_table(sample_rate),
            hrtf_states: Vec::new(),
            sample_rate,
        }
    }

    pub fn enable_hrtf(&mut self) {
        self.hrtf_enabled = true;
    }
    pub fn disable_hrtf(&mut self) {
        self.hrtf_enabled = false;
    }

    pub fn add_source(&mut self, position: [f32; 3]) -> Option<u64> {
        if self.sources.len() >= self.max_sources {
            return None;
        }
        let id = self.sources.len() as u64;
        self.sources.push(AudioSource3D::new(id, position));
        self.hrtf_states.push(HrtfSourceState::new(id));
        Some(id)
    }

    pub fn remove_source(&mut self, id: u64) {
        self.sources.retain(|s| s.id != id);
        self.hrtf_states.retain(|s| s.source_id != id);
    }

    pub fn set_source_position(&mut self, source_id: u64, x: f32, y: f32, z: f32) {
        if let Some(src) = self.sources.iter_mut().find(|s| s.id == source_id) {
            src.position = [x, y, z];
        }
    }

    pub fn update_source_position(&mut self, id: u64, position: [f32; 3]) {
        if let Some(src) = self.sources.iter_mut().find(|s| s.id == id) {
            src.position = position;
        }
    }

    pub fn update_source_velocity(&mut self, id: u64, velocity: [f32; 3]) {
        if let Some(src) = self.sources.iter_mut().find(|s| s.id == id) {
            src.velocity = velocity;
        }
    }

    pub fn set_listener_position(&mut self, position: [f32; 3]) {
        self.listener.position = position;
    }

    pub fn set_listener_orientation(&mut self, forward: [f32; 3], up: [f32; 3]) {
        self.listener.forward = forward;
        self.listener.up = up;
    }

    /// Compute azimuth angle (0–360°) from listener to source in the horizontal plane.
    fn azimuth_to(&self, src: &AudioSource3D) -> f32 {
        let dx = src.position[0] - self.listener.position[0];
        let dz = src.position[2] - self.listener.position[2];
        let fwd = self.listener.forward;
        let right = self.listener.right();

        let proj_fwd = dx * fwd[0] + dz * fwd[2];
        let proj_right = dx * right[0] + dz * right[2];

        let angle = atan2_fast(proj_right, proj_fwd) * (180.0 / core::f32::consts::PI);
        if angle < 0.0 {
            angle + 360.0
        } else {
            angle
        }
    }

    /// Look up HRTF coefficients for a given azimuth (quantized to 5° bins).
    fn hrtf_lookup(&self, azimuth_deg: f32) -> &HrtfCoeffs {
        let idx = ((azimuth_deg / 5.0) as usize) % HRTF_DIRECTIONS;
        &self.hrtf_table[idx]
    }

    /// Process mono source samples through HRTF, writing stereo output.
    /// This is the hot path — runs every 2.67ms audio frame.
    pub fn process_hrtf_source(
        &mut self,
        source_idx: usize,
        mono_in: &[f32],
        stereo_out: &mut [f32],
    ) {
        let src = match self.sources.get(source_idx) {
            Some(s) => s,
            None => return,
        };

        let att = src.attenuation(&self.listener);
        if att < 0.001 {
            return;
        }

        let azimuth = self.azimuth_to(src);
        let coeffs = *self.hrtf_lookup(azimuth);

        let state = match self.hrtf_states.get_mut(source_idx) {
            Some(s) => s,
            None => return,
        };

        let delay_mask = 63; // delay line is 64 samples

        for (i, &sample) in mono_in.iter().enumerate() {
            let s = sample * att;

            // Write into delay lines
            state.delay_line_left[state.delay_write_pos] = s * coeffs.ild_left;
            state.delay_line_right[state.delay_write_pos] = s * coeffs.ild_right;

            // Read from delay lines with ITD offset
            let left_delay = coeffs.itd_samples_left as usize;
            let right_delay = coeffs.itd_samples_right as usize;
            let left_read = (state.delay_write_pos + 64 - left_delay) & delay_mask;
            let right_read = (state.delay_write_pos + 64 - right_delay) & delay_mask;

            let delayed_l = state.delay_line_left[left_read];
            let delayed_r = state.delay_line_right[right_read];

            // Apply FIR filter (shift history, convolve)
            for t in (1..HRTF_FILTER_TAPS).rev() {
                state.fir_history_left[t] = state.fir_history_left[t - 1];
                state.fir_history_right[t] = state.fir_history_right[t - 1];
            }
            state.fir_history_left[0] = delayed_l;
            state.fir_history_right[0] = delayed_r;

            let mut out_l = 0.0f32;
            let mut out_r = 0.0f32;
            for t in 0..HRTF_FILTER_TAPS {
                out_l += state.fir_history_left[t] * coeffs.left_fir[t];
                out_r += state.fir_history_right[t] * coeffs.right_fir[t];
            }

            let out_idx = i * 2;
            if out_idx + 1 < stereo_out.len() {
                stereo_out[out_idx] += out_l;
                stereo_out[out_idx + 1] += out_r;
            }

            state.delay_write_pos = (state.delay_write_pos + 1) & delay_mask;
        }
    }

    /// Non-HRTF fallback: simple stereo panning with distance attenuation.
    pub fn compute_gains(&self) -> Vec<(u64, f32, f32)> {
        let mut gains = Vec::with_capacity(self.sources.len());
        for src in &self.sources {
            let att = src.attenuation(&self.listener);
            let (left, right) = src.stereo_pan(&self.listener);
            gains.push((src.id, att * left, att * right));
        }
        gains
    }

    pub fn active_sources(&self) -> usize {
        self.sources.len()
    }
}

// ─── Fast trig for no_std (integer math, no libm) ───────────────────────────

fn sin_fast(x: f32) -> f32 {
    // Normalize to [-π, π]
    let mut a = x;
    while a > core::f32::consts::PI {
        a -= 2.0 * core::f32::consts::PI;
    }
    while a < -core::f32::consts::PI {
        a += 2.0 * core::f32::consts::PI;
    }
    // Degree-5 Chebyshev approximation
    let a2 = a * a;
    a * (1.0 - a2 / 6.0 * (1.0 - a2 / 20.0))
}

fn cos_fast(x: f32) -> f32 {
    sin_fast(x + core::f32::consts::FRAC_PI_2)
}

fn atan2_fast(y: f32, x: f32) -> f32 {
    if x == 0.0 && y == 0.0 {
        return 0.0;
    }
    let abs_x = if x < 0.0 { -x } else { x };
    let abs_y = if y < 0.0 { -y } else { y };

    let (a, offset) = if abs_x >= abs_y {
        let r = abs_y / abs_x;
        (r - r * r * r * 0.1667, 0.0f32)
    } else {
        let r = abs_x / abs_y;
        (
            core::f32::consts::FRAC_PI_2 - (r - r * r * r * 0.1667),
            0.0f32,
        )
    };
    let _ = offset;

    let angle = a;
    let angle = if x < 0.0 {
        core::f32::consts::PI - angle
    } else {
        angle
    };
    if y < 0.0 {
        -angle
    } else {
        angle
    }
}

// ─── Audio Manager ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LatencyMode {
    UltraLow,
    Low,
    Normal,
    High,
}

impl LatencyMode {
    pub fn buffer_size_frames(&self, sample_rate: u32) -> u32 {
        match self {
            LatencyMode::UltraLow => sample_rate / 1000, // ~1ms
            LatencyMode::Low => sample_rate / 200,       // ~5ms
            LatencyMode::Normal => sample_rate / 100,    // ~10ms
            LatencyMode::High => sample_rate / 25,       // ~40ms
        }
    }

    pub fn latency_us(&self) -> u32 {
        match self {
            LatencyMode::UltraLow => 1000,
            LatencyMode::Low => 5000,
            LatencyMode::Normal => 10000,
            LatencyMode::High => 40000,
        }
    }
}

pub struct AudioManager {
    pub hda: Option<HdaController>,
    pub mixer: AudioMixer,
    pub router: AudioRouter,
    pub spatial: SpatialAudioEngine,
    pub latency_mode: LatencyMode,
    pub sample_rate: u32,
    pub buffer_size_frames: u32,
    pub playback_stream_id: Option<u8>,
    pub capture_stream_id: Option<u8>,
    pub capture_session: Option<CaptureSession>,
    pub loopback: LoopbackCapture,
}

impl AudioManager {
    pub fn new() -> Self {
        let sample_rate = 48000;
        let latency = LatencyMode::Normal;
        let format = AudioStreamFormat::DVD_QUALITY;

        let mut router = AudioRouter::new();

        router.add_physical_output("Speakers", EndpointType::Speaker, format);
        router.add_physical_output("Headphones", EndpointType::Headphone, format);
        router.add_physical_input("Microphone", EndpointType::Microphone, format);

        router.add_virtual_output("System Audio", 2, sample_rate);
        router.add_virtual_output("Communication", 2, sample_rate);
        router.add_virtual_input("Virtual Mic", 2, sample_rate);

        Self {
            hda: None,
            mixer: AudioMixer::new(format),
            router,
            spatial: SpatialAudioEngine::new(64),
            latency_mode: latency,
            sample_rate,
            buffer_size_frames: latency.buffer_size_frames(sample_rate),
            playback_stream_id: None,
            capture_stream_id: None,
            capture_session: None,
            loopback: LoopbackCapture::new(RING_BUFFER_SAMPLES),
        }
    }

    pub fn init_hda(&mut self, base_phys: u64) -> Result<(), &'static str> {
        let mut hda = HdaController::new(base_phys);
        hda.init_controller()?;

        let playback_id =
            hda.setup_stream(StreamDirection::Playback, AudioStreamFormat::DVD_QUALITY)?;
        let capture_id =
            hda.setup_stream(StreamDirection::Capture, AudioStreamFormat::DVD_QUALITY)?;

        self.playback_stream_id = Some(playback_id);
        self.capture_stream_id = Some(capture_id);
        self.hda = Some(hda);
        Ok(())
    }

    pub fn set_latency_mode(&mut self, mode: LatencyMode) {
        self.latency_mode = mode;
        self.buffer_size_frames = mode.buffer_size_frames(self.sample_rate);
    }

    pub fn set_sample_rate(&mut self, rate: u32) {
        self.sample_rate = rate;
        self.buffer_size_frames = self.latency_mode.buffer_size_frames(rate);
    }

    pub fn register_app(&mut self, pid: u64, name: &str) {
        self.mixer.register_app(pid, name);
    }

    pub fn unregister_app(&mut self, pid: u64) {
        self.mixer.unregister_app(pid);
    }

    pub fn set_master_volume(&mut self, volume: f32) {
        self.mixer.set_master_volume(volume);
    }

    pub fn set_app_volume(&mut self, pid: u64, volume: f32) {
        self.mixer.set_app_volume(pid, volume);
    }

    /// PRODUCER tick, driven once per period by the SCHED_BODY audio thread.
    /// Mixes all registered voices into `mixer.mix_buffer`, feeds the
    /// loopback monitor, then pushes the mixed samples into AUDIO_RING. The
    /// drain side of the audio thread is the sole ring consumer (SPSC).
    ///
    /// Returns the number of f32 samples actually written into the ring this
    /// period (0 when the mixer produced silence or the ring was full).
    pub fn process_tick(&mut self) -> usize {
        self.mixer.mix();
        self.mixer.update_levels();
        self.loopback.feed(&self.mixer.mix_buffer);

        // Only publish the frames the mixer actually produced this period.
        let produced =
            (self.mixer.last_mixed_frames * AUDIO_CHANNELS).min(self.mixer.mix_buffer.len());
        if produced == 0 {
            return 0;
        }
        match AUDIO_RING.get() {
            Some(ring) => ring.write(&self.mixer.mix_buffer[..produced]),
            None => 0,
        }
    }

    pub fn codec_info(&self) -> Vec<(&str, u32)> {
        let mut info = Vec::new();
        if let Some(hda) = &self.hda {
            for codec in &hda.codecs {
                info.push((codec.vendor_name(), codec.vendor_id));
            }
        }
        info
    }

    pub fn stream_count(&self) -> (usize, usize) {
        if let Some(hda) = &self.hda {
            let playback = hda
                .streams
                .iter()
                .filter(|s| s.direction == StreamDirection::Playback)
                .count();
            let capture = hda
                .streams
                .iter()
                .filter(|s| s.direction == StreamDirection::Capture)
                .count();
            (playback, capture)
        } else {
            (0, 0)
        }
    }
}

// ─── SCHED_BODY Audio Thread ────────────────────────────────────────────────
//
// Runs at TaskPriority::Game — the highest real-time priority class.
// Wakes on timer tick (in production: HDA IOC interrupt), reads mixed
// samples from the ring buffer, converts f32 → i16, and copies them
// into the DMA buffer for the HDA stream descriptor.  Target latency:
// 128 frames @ 48 kHz = ~2.67 ms per period.

extern "C" fn audio_thread_entry() {
    let mut read_buf = [0.0f32; AUDIO_PERIOD_SAMPLES];
    let mut i16_buf = [0i16; AUDIO_PERIOD_SAMPLES];

    loop {
        // ── PRODUCER: mix registered voices → AUDIO_RING ──────────────────
        // Take AUDIO_MANAGER (interrupts-disabled via lock_audio — single-CPU
        // IF=0 deadlock guard), run one mixer period, drop the lock BEFORE any
        // hlt() below (the guard is never held across a yield — matches the drain
        // pattern; its drop re-enables interrupts so we halt with IF=1).
        {
            let mut guard = lock_audio();
            if let Some(mgr) = guard.as_mut() {
                let wrote = mgr.process_tick();
                if wrote > 0 {
                    AUDIO_PRODUCER_TICKS.fetch_add(1, Ordering::Relaxed);
                    AUDIO_PRODUCER_SAMPLES.fetch_add(wrote, Ordering::Relaxed);
                }
            }
        } // guard dropped here

        // ── CONSUMER: drain AUDIO_RING → HDA DMA buffer ───────────────────
        let samples_ready = AUDIO_RING.get().map(|r| r.available_read()).unwrap_or(0);

        if samples_ready >= AUDIO_PERIOD_SAMPLES {
            if let Some(ring) = AUDIO_RING.get() {
                let got = ring.read(&mut read_buf);
                if got > 0 {
                    f32_to_i16(&read_buf[..got], &mut i16_buf[..got]);

                    // Copy i16 samples into the HDA DMA buffer (interrupts-disabled
                    // via lock_audio; dropped before the loop's hlt()).
                    let mgr = lock_audio();
                    if let Some(ref mgr) = *mgr {
                        if let Some(ref hda) = mgr.hda {
                            if let Some(pid) = mgr.playback_stream_id {
                                if let Some(stream) = hda.streams.get(pid as usize) {
                                    if stream.running && stream.dma_virt != 0 {
                                        let pos = unsafe {
                                            hda.mmio_read32(hda.stream_reg_base(pid) + HDA_SD_LPIB)
                                        };
                                        let byte_count = (got * 2) as u32;
                                        let write_off = pos % stream.dma_size;
                                        let dst = (stream.dma_virt + write_off as u64) as *mut i16;
                                        let to_copy = byte_count.min(stream.dma_size - write_off)
                                            as usize
                                            / 2;
                                        unsafe {
                                            core::ptr::copy_nonoverlapping(
                                                i16_buf.as_ptr(),
                                                dst,
                                                to_copy,
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Yield THEN halt — never a bare `hlt()`. As a SCHED_BODY deadline
        // (EDF) task this MUST call `yield_task()` so the scheduler runs
        // `dl.finish()` and the period is released to the Normal/CFS class until
        // the next 2.667 ms boundary. A bare `hlt()` here would halt the CPU
        // while this task is still `current` and un-`finish()`ed, so it would
        // keep winning `pick_next` (deadline class is evaluated before Normal)
        // and starve every Normal task on this CPU — the same defect as the
        // 360b862 xHCI HID idle path. The trailing `hlt()` parks the CPU until
        // the next timer tick / HDA IOC interrupt if yield_task handed us back.
        crate::scheduler::yield_task();
        x86_64::instructions::hlt();
    }
}

fn spawn_audio_thread() {
    let mut task = crate::task::Task::new(audio_thread_entry, None);
    task.priority = crate::task::TaskPriority::Game;
    task.deadline = Some(crate::task::DeadlineTask::new(2_667, 2_000, 1_500));
    let task_id = task.id;
    crate::scheduler::spawn(task);
    let _ = crate::scheduler::configure_audio_deadline(task_id);
}

// ─── Global State ───────────────────────────────────────────────────────────

pub static AUDIO_MANAGER: Mutex<Option<AudioManager>> = Mutex::new(None);

/// RAII guard returned by [`lock_audio`]: holds the `AUDIO_MANAGER` spin lock
/// with interrupts disabled for the whole critical section, restoring the
/// previous interrupt state on drop.
///
/// SINGLE-CPU DEADLOCK GUARD (flagged by raeen-reviewer when SYS_AUDIO_SUBMIT
/// (267) made the window reachable; same class as compositor.rs `lock_compositor`,
/// root-caused iron 2026-06-15). `AUDIO_MANAGER` is shared between a preemptible
/// kernel thread (the SCHED_BODY audio thread's `process_tick` producer + the DMA
/// drain consumer) and syscall handlers — `submit_samples` (← SYS_AUDIO_SUBMIT)
/// and `dump_text` (← /proc/raeen/audio read) — which run with `RFLAGS.IF=0`
/// (SFMASK clears it on SYSCALL entry). On this kernel only the BSP schedules
/// post-boot (APs halt — see scheduler::ap_enter_idle), so a spinning IF=0 waiter
/// can NEVER be preempted. If the audio thread were preempted while holding the
/// lock and the scheduler picked a userspace task that called SYS_AUDIO_SUBMIT,
/// that IF=0 syscall would spin forever on the held lock (the holder can never
/// resume). Disabling interrupts for the entire hold makes every critical section
/// atomic w.r.t. every other, so a waiter always finds the lock free.
///
/// Latency cost (acceptable): the audio thread now mixes one period (256 samples,
/// ~2.67 ms @ 48 kHz) with interrupts disabled — a bounded short window, exactly
/// like the compositor composites a frame under IF=0. Fine for single-CPU lock
/// safety until APs schedule. The guard MUST be dropped before `hlt()` so the
/// thread halts with interrupts ENABLED (the drop re-enables IF).
struct AudioGuard {
    guard: Option<spin::MutexGuard<'static, Option<AudioManager>>>,
    was_enabled: bool,
}

impl core::ops::Deref for AudioGuard {
    type Target = Option<AudioManager>;
    fn deref(&self) -> &Self::Target {
        self.guard.as_ref().unwrap()
    }
}

impl core::ops::DerefMut for AudioGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.guard.as_mut().unwrap()
    }
}

impl Drop for AudioGuard {
    fn drop(&mut self) {
        // Release the spin lock FIRST, then restore interrupts — never the
        // reverse, or an IRQ between unlock and re-enable could observe a
        // half-torn state. Mirrors compositor.rs CompositorGuard::drop exactly.
        self.guard = None;
        if self.was_enabled {
            x86_64::instructions::interrupts::enable();
        }
    }
}

/// Acquire `AUDIO_MANAGER` with interrupts disabled. Use this everywhere instead
/// of `AUDIO_MANAGER.lock()` — see [`AudioGuard`] for why (single-CPU IF=0
/// deadlock avoidance). Mirrors compositor.rs `lock_compositor`.
#[inline]
fn lock_audio() -> AudioGuard {
    let was_enabled = x86_64::instructions::interrupts::are_enabled();
    x86_64::instructions::interrupts::disable();
    AudioGuard {
        guard: Some(AUDIO_MANAGER.lock()),
        was_enabled,
    }
}

/// Quick Settings / Control Center accessor: the live master volume in
/// `0.0..=1.0` (Concept §Unified Settings — "every control reaches the real
/// engine"). Reads through `lock_audio` so it is atomic w.r.t. the SCHED_BODY
/// audio thread. Returns the pre-boot default (0.8) before the mixer is up.
#[must_use]
pub fn quick_master_volume() -> f32 {
    lock_audio()
        .as_ref()
        .map(|m| m.mixer.master_volume)
        .unwrap_or(0.8)
}

/// Quick Settings / Control Center mutator: set the live master volume
/// (clamped 0.0..=1.0 by the mixer). The notification-center mute toggle drives
/// this — mute stashes the prior level and sets 0.0; unmute restores it. Routed
/// through the real `AudioManager`, so the change is audible, not cosmetic.
pub fn quick_set_master_volume(volume: f32) {
    if let Some(m) = lock_audio().as_mut() {
        m.set_master_volume(volume);
    }
}

pub static AUDIO_RING: spin::Once<AudioRingBuffer> = spin::Once::new();
static TEST_TONE_WRITES: AtomicUsize = AtomicUsize::new(0);
static TEST_TONE_SAMPLES_WRITTEN: AtomicUsize = AtomicUsize::new(0);
/// Periods in which the mixer produced samples into AUDIO_RING (producer ran).
static AUDIO_PRODUCER_TICKS: AtomicUsize = AtomicUsize::new(0);
/// Total f32 samples the mixer producer has pushed into AUDIO_RING.
static AUDIO_PRODUCER_SAMPLES: AtomicUsize = AtomicUsize::new(0);

pub fn init() {
    AUDIO_RING.call_once(|| AudioRingBuffer::new(RING_BUFFER_SAMPLES));
    CAPTURE_RING.call_once(|| AudioRingBuffer::new(RING_BUFFER_SAMPLES));

    let mut mgr = AudioManager::new();

    if let Some(bar_phys) = find_hda_bar() {
        crate::pci::enable_bus_mastering(
            &crate::pci::enumerate()
                .iter()
                .find(|d| d.class == 0x04 && d.subclass == 0x03)
                .cloned()
                .unwrap(),
        );
        match mgr.init_hda(bar_phys) {
            Ok(()) => {
                crate::serial_println!(
                    "[audio] HDA controller initialised at BAR {:#010x}",
                    bar_phys
                );
                if let Some(ref mut hda) = mgr.hda {
                    if let Some(pid) = mgr.playback_stream_id {
                        let _ = hda.start_stream(pid);
                    }
                }
            }
            Err(e) => crate::serial_println!("[audio] HDA init failed: {}", e),
        }
    } else {
        crate::serial_println!("[audio] no HDA controller found on PCI bus");
    }

    *lock_audio() = Some(mgr);

    spawn_audio_thread();
    crate::serial_println!(
        "[audio] SCHED_BODY audio thread spawned (period={}frames, ~2.67ms)",
        AUDIO_PERIOD_FRAMES
    );
}

/// Total f32 samples submitted by apps via `submit_samples` (telemetry).
static AUDIO_SUBMIT_SAMPLES: AtomicUsize = AtomicUsize::new(0);
/// Total successful `submit_samples` calls (telemetry).
static AUDIO_SUBMIT_CALLS: AtomicUsize = AtomicUsize::new(0);

/// Feed app PCM into the mixer (backs `SYS_AUDIO_SUBMIT`, 267). `samples` is
/// interleaved 48 kHz i16-stereo converted to f32 in [-1, 1]; `pid` is the
/// calling task. Registers/feeds the task's `SourceKind::Pcm` voice. Returns
/// `Some(frames_accepted)` (frames = samples / 2), or `None` if audio isn't
/// initialised or the per-app voice cap is hit. No allocation on the steady
/// path — the per-source queue is sized once.
pub fn submit_samples(samples: &[i16], pid: u64) -> Option<usize> {
    // IF=0 caller (SYS_AUDIO_SUBMIT): lock_audio keeps the acquisition atomic
    // w.r.t. the preemptible audio thread so this can never spin on a held lock.
    let mut guard = lock_audio();
    let mgr = guard.as_mut()?;
    // i16 → f32 in [-1, 1]. Bounded by AUDIO_SUBMIT_MAX_FRAMES at the syscall
    // edge, so this stack→heap copy is small. We convert into a scratch Vec;
    // the enqueue itself is allocation-free into the persistent source ring.
    let mut f = Vec::with_capacity(samples.len());
    for &s in samples {
        f.push(s as f32 / 32768.0);
    }
    let accepted = mgr.mixer.submit_pcm(pid, &f)?;
    if accepted > 0 {
        AUDIO_SUBMIT_CALLS.fetch_add(1, Ordering::Relaxed);
        AUDIO_SUBMIT_SAMPLES.fetch_add(accepted, Ordering::Relaxed);
    }
    Some(accepted / AUDIO_CHANNELS)
}

/// Drop every PCM mixer voice owned by `pid`. Called from task-exit reclaim so
/// a dead app's voice doesn't stream silence forever. Cheap no-op when the task
/// never made sound. Returns sources removed.
pub fn remove_task_sources(pid: u64) -> usize {
    let mut guard = lock_audio();
    match guard.as_mut() {
        Some(mgr) => mgr.mixer.remove_task_sources(pid),
        None => 0,
    }
}

/// The mixer/ring canonical output format: stereo interleaved f32 at 48 kHz —
/// every producer (mixer voices, the test tone, SYS_AUDIO_SUBMIT) feeds
/// AUDIO_RING in this shape.
pub const OUTPUT_RATE_HZ: u32 = 48_000;
const OUTPUT_CHANNELS: usize = 2;

/// PERFORMANCE_TARGETS §3 (sub-3ms round-trip) — the OUTPUT-PATH latency
/// instrument: a sample written to AUDIO_RING right now sits behind
/// `backlog_frames` of queued audio, so its wait before the HDA DMA engine
/// consumes it is `backlog ÷ rate`. Returns `(backlog_frames, rate_hz,
/// latency_us)`. The full round-trip on iron = this + the codec/DAC path;
/// QEMU proves the INSTRUMENT (HDA consumption itself is iron-proven
/// separately), and the next iron boot reads the real steady-state number
/// from `/proc/raeen/audio`.
pub fn output_latency_snapshot() -> (usize, u32, u64) {
    let backlog_frames = AUDIO_RING
        .get()
        .map(|r| r.available_read() / OUTPUT_CHANNELS)
        .unwrap_or(0);
    let latency_us = (backlog_frames as u64) * 1_000_000 / OUTPUT_RATE_HZ as u64;
    (backlog_frames, OUTPUT_RATE_HZ, latency_us)
}

fn enqueue_test_tone_square(frames: usize, sample_rate: u32, hz: u32) -> usize {
    let Some(ring) = AUDIO_RING.get() else {
        return 0;
    };
    let period = (sample_rate / hz.max(1)).max(2) as usize;
    let mut samples = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        let phase = i % period;
        let v = if phase < (period / 2) {
            0.18_f32
        } else {
            -0.18_f32
        };
        samples.push(v);
        samples.push(v);
    }
    let wrote = ring.write(&samples);
    if wrote > 0 {
        TEST_TONE_WRITES.fetch_add(1, Ordering::Relaxed);
        TEST_TONE_SAMPLES_WRITTEN.fetch_add(wrote, Ordering::Relaxed);
    }
    // /proc/raeen/perf telemetry: one period attempt; a fully-full ring
    // (wrote == 0) means the HDA DMA consumer hasn't drained — a starvation /
    // underrun-class event the sub-3ms contract cares about.
    crate::perf::record_audio_period(wrote == 0);
    wrote
}

pub fn run_boot_smoketest() {
    let wrote = enqueue_test_tone_square(480, OUTPUT_RATE_HZ, 440);
    let hda_playback = lock_audio()
        .as_ref()
        .map(|m| m.playback_stream_id.is_some())
        .unwrap_or(false);
    crate::serial_println!(
        "[audio] test-tone smoketest: wrote_samples={} hda_playback={} -> {}",
        wrote,
        hda_playback as u8,
        if wrote > 0 { "PASS" } else { "FAIL" }
    );

    // Output-latency instrument (§3 sub-3ms round-trip): with the 480-frame
    // (10 ms) tone just queued the backlog MUST be nonzero and the computed
    // wait sane — a zero backlog (ring not wired), a zero latency (bad
    // divide), or a >1 s figure (cursor corruption) all FAIL. The contract
    // NUMBER is read on iron; this proves the instrument can't lie silently.
    let (backlog_frames, rate, lat_us) = output_latency_snapshot();
    let lat_ok = backlog_frames > 0 && lat_us > 0 && lat_us < 1_000_000;
    crate::serial_println!(
        "[audio] output-latency instrument: backlog={} frames @ {} Hz -> {}us queued (iron round-trip contract <3000us) -> {}",
        backlog_frames,
        rate,
        lat_us,
        if lat_ok { "PASS" } else { "FAIL" }
    );

    // ── Mixer producer smoketest (Phase 7) ───────────────────────────────
    // Prove the mixer→ring PRODUCER path is wired: register the square-wave
    // test tone as a real MIXER SOURCE (a voice the mixer reads), run N
    // process_tick() cycles, and assert the mixer produced samples INTO
    // AUDIO_RING. This asserts the producer RUNS and FILLS the ring — it is
    // QEMU-deterministic and does NOT depend on HDA DMA / LPIB consumption
    // (which is uncertain under QEMU; proven only on iron, separately).
    //
    // FAIL-able: if process_tick wired nothing (sources never mixed, or the
    // ring write never happened), mixed_frames stays 0 and ring_filled=false.
    const MIXER_TEST_TICKS: usize = 8;
    let (sources, mixed_samples, last_frames) = {
        let mut guard = lock_audio();
        if let Some(mgr) = guard.as_mut() {
            // Drain any residue so the producer writes are what we measure.
            if let Some(ring) = AUDIO_RING.get() {
                let mut sink = [0.0f32; AUDIO_PERIOD_SAMPLES];
                while ring.available_read() > 0 {
                    if ring.read(&mut sink) == 0 {
                        break;
                    }
                }
            }
            let src_id = mgr.mixer.register_test_tone(OUTPUT_RATE_HZ, 440);
            let sources = mgr.mixer.active_source_count();
            // Run the producer N periods, summing samples it pushes to the ring.
            // We drain between ticks so a full ring never throttles the proof.
            let mut total = 0usize;
            let mut sink = [0.0f32; AUDIO_PERIOD_SAMPLES];
            for _ in 0..MIXER_TEST_TICKS {
                total += mgr.process_tick();
                if let Some(ring) = AUDIO_RING.get() {
                    let _ = ring.read(&mut sink);
                }
            }
            let last_frames = mgr.mixer.last_mixed_frames;
            // Keep the boot tone one-shot: remove the source now that the
            // producer path is proven (real app voices register at runtime).
            mgr.mixer.remove_source(src_id);
            (sources, total, last_frames)
        } else {
            (0, 0, 0)
        }
    };
    let expected = MIXER_TEST_TICKS * AUDIO_PERIOD_SAMPLES;
    let ring_filled = mixed_samples >= expected && last_frames == AUDIO_PERIOD_FRAMES;
    let mixer_pass = sources == 1 && ring_filled;
    crate::serial_println!(
        "[audio] mixer smoketest: sources={} mixed_frames={} ring_filled={} -> {}",
        sources,
        mixed_samples / AUDIO_CHANNELS,
        ring_filled,
        if mixer_pass { "PASS" } else { "FAIL" }
    );

    // ── App PCM submit smoketest (Phase 7, SYS_AUDIO_SUBMIT) ──────────────
    // Prove app → mixer → ring: submit a synthetic i16 buffer for a fake pid
    // via the SAME submit_samples path the syscall uses, run process_tick(),
    // and assert (a) the submit registered a PCM source that queued the
    // samples, and (b) the mixer pulled from it into AUDIO_RING. FAIL-able: if
    // submit_pcm never created a voice (registered=false) or the mixer didn't
    // drain it (ring_advanced=false / source never went active), it prints FAIL.
    const SUBMIT_TEST_PID: u64 = 0xFEED_BEEF;
    const SUBMIT_TEST_FRAMES: usize = AUDIO_PERIOD_FRAMES; // one period
    let (registered, mixed_samples, queue_drained) = {
        // Drain residue so the ring fill we measure is the mixer's PCM output.
        if let Some(ring) = AUDIO_RING.get() {
            let mut sink = [0.0f32; AUDIO_PERIOD_SAMPLES];
            while ring.available_read() > 0 {
                if ring.read(&mut sink) == 0 {
                    break;
                }
            }
        }
        // Synthetic interleaved i16-stereo buffer (a small ramp, nonzero).
        let mut buf = Vec::with_capacity(SUBMIT_TEST_FRAMES * AUDIO_CHANNELS);
        for f in 0..SUBMIT_TEST_FRAMES {
            let v = ((f as i32 % 64) * 256) as i16;
            buf.push(v);
            buf.push(v);
        }
        let accepted = submit_samples(&buf, SUBMIT_TEST_PID).unwrap_or(0);
        let queued_after_submit = lock_audio()
            .as_ref()
            .map(|m| m.mixer.pcm_queued_for(SUBMIT_TEST_PID))
            .unwrap_or(0);
        let registered = accepted == SUBMIT_TEST_FRAMES && queued_after_submit > 0;

        // Run one producer period: the mixer must drain the PCM queue into the ring.
        let mixed = {
            let mut guard = lock_audio();
            guard.as_mut().map(|m| m.process_tick()).unwrap_or(0)
        };
        let queued_after_tick = lock_audio()
            .as_ref()
            .map(|m| m.mixer.pcm_queued_for(SUBMIT_TEST_PID))
            .unwrap_or(usize::MAX);
        // Drain the ring residue + remove the synthetic voice so the boot path
        // is left clean (real app voices register at runtime).
        if let Some(ring) = AUDIO_RING.get() {
            let mut sink = [0.0f32; AUDIO_PERIOD_SAMPLES];
            let _ = ring.read(&mut sink);
        }
        let removed = remove_task_sources(SUBMIT_TEST_PID);
        (
            registered,
            mixed,
            queued_after_submit > queued_after_tick && removed == 1,
        )
    };
    let submit_mixed = mixed_samples >= SUBMIT_TEST_FRAMES * AUDIO_CHANNELS;
    let submit_pass = registered && submit_mixed && queue_drained;
    crate::serial_println!(
        "[audio] submit smoketest: frames_accepted={} mixed={} -> {}",
        if registered { SUBMIT_TEST_FRAMES } else { 0 },
        submit_mixed,
        if submit_pass { "PASS" } else { "FAIL" }
    );

    // Phase 7.1 codec walk: with a REAL HDA controller present (QEMU
    // intel-hda + hda-output codec, or iron), the CORB/RIRB verb walk must
    // have found a codec whose widget graph carries a DAC and an output
    // pin — the topology PCM playback wires through.
    {
        let guard = lock_audio();
        match guard.as_ref().and_then(|m| m.hda.as_ref()) {
            Some(hda) => {
                let codecs = hda.codecs.len();
                let dac = hda.codecs.iter().any(|c| c.find_dac().is_some());
                let out_pin = hda.codecs.iter().any(|c| !c.output_pins().is_empty());
                let widgets: usize = hda.codecs.iter().map(|c| c.nodes.len()).sum();
                let pass = codecs >= 1 && dac && out_pin && widgets >= 2;
                crate::serial_println!(
                "[audio] codec-walk smoketest: codecs={} widgets={} dac={} output_pin={} stream_armed={} -> {}",
                codecs,
                widgets,
                dac,
                out_pin,
                hda_playback,
                if pass { "PASS" } else { "FAIL" },
            );
            }
            None => {
                crate::serial_println!(
                    "[audio] codec-walk smoketest: no HDA controller on this machine -> SKIP"
                );
            }
        }
    } // codec-walk AudioGuard dropped here (re-enables interrupts) before lock_audio re-entry below

    // ── lock_audio interrupt-discipline smoketest (single-CPU deadlock guard) ──
    // Prove the AudioGuard saves/restores RFLAGS.IF correctly: interrupts must be
    // DISABLED while the guard is held, and RESTORED to their prior value after
    // the guard drops. This is the exact invariant that closes the SYS_AUDIO_SUBMIT
    // (267) IF=0 deadlock window — getting the save/restore order wrong would
    // either reintroduce the bug or strand interrupts disabled.
    //
    // FAIL-able: if lock_audio() forgot to `cli`, if_disabled_held=false; if the
    // guard's Drop failed to restore the prior IF, if_restored=false. We run it
    // from a known-interrupts-ENABLED state (the boot smoketest path) so "restore
    // to prior" means re-enabled.
    {
        let prior_if = x86_64::instructions::interrupts::are_enabled();
        // Force a deterministic, known-enabled baseline so the proof is meaningful
        // regardless of how we were entered; restore is asserted against this.
        x86_64::instructions::interrupts::enable();
        let if_disabled_held;
        {
            let _g = lock_audio();
            // Inside the held guard interrupts MUST be off.
            if_disabled_held = !x86_64::instructions::interrupts::are_enabled();
        } // _g dropped: lock released, then IF restored to the prior (enabled) state
        let if_restored = x86_64::instructions::interrupts::are_enabled();
        // Leave the interrupt flag as we found it on entry to the smoketest.
        if !prior_if {
            x86_64::instructions::interrupts::disable();
        }
        let lock_pass = if_disabled_held && if_restored;
        crate::serial_println!(
            "[audio] lock_audio smoketest: if_disabled_held={} if_restored={} -> {}",
            if_disabled_held,
            if_restored,
            if lock_pass { "PASS" } else { "FAIL" }
        );
    }
}

pub fn dump_text() -> String {
    let mut out = String::new();
    out.push_str("# AthenaOS audio subsystem\n");
    if let Some(mgr) = lock_audio().as_ref() {
        let (playback, capture) = mgr.stream_count();
        out.push_str(&alloc::format!(
            "hda_present: {}\nplayback_streams: {}\ncapture_streams: {}\n",
            mgr.hda.is_some() as u8,
            playback,
            capture
        ));
    } else {
        out.push_str("hda_present: 0\nplayback_streams: 0\ncapture_streams: 0\n");
    }
    out.push_str(&alloc::format!(
        "test_tone_writes: {}\ntest_tone_samples_written: {}\n",
        TEST_TONE_WRITES.load(Ordering::Relaxed),
        TEST_TONE_SAMPLES_WRITTEN.load(Ordering::Relaxed)
    ));
    // Mixer producer telemetry: periods in which the SCHED_BODY thread mixed
    // registered voices into AUDIO_RING, and total samples published. Nonzero
    // here means audio is flowing through the real mixer→ring path.
    let active_sources = lock_audio()
        .as_ref()
        .map(|m| m.mixer.active_source_count())
        .unwrap_or(0);
    out.push_str(&alloc::format!(
        "mixer_active_sources: {}\nmixer_producer_ticks: {}\nmixer_producer_samples: {}\n",
        active_sources,
        AUDIO_PRODUCER_TICKS.load(Ordering::Relaxed),
        AUDIO_PRODUCER_SAMPLES.load(Ordering::Relaxed)
    ));
    // App PCM submit telemetry (SYS_AUDIO_SUBMIT path): nonzero means apps are
    // feeding the mixer directly through the syscall edge.
    out.push_str(&alloc::format!(
        "audio_submit_calls: {}\naudio_submit_samples: {}\n",
        AUDIO_SUBMIT_CALLS.load(Ordering::Relaxed),
        AUDIO_SUBMIT_SAMPLES.load(Ordering::Relaxed)
    ));
    // Output-path latency (§3 sub-3ms round-trip): ring backlog ÷ rate = the
    // wait a fresh sample faces before HDA DMA consumes it. THE number the
    // contract is judged by on iron.
    let (backlog_frames, rate, lat_us) = output_latency_snapshot();
    out.push_str(&alloc::format!(
        "output_backlog_frames: {}\noutput_rate_hz: {}\noutput_latency_us: {}\n",
        backlog_frames,
        rate,
        lat_us
    ));
    out
}

fn find_hda_bar() -> Option<u64> {
    let pci_devices = crate::pci::enumerate();
    for dev in &pci_devices {
        if dev.class == 0x04 && dev.subclass == 0x03 {
            let raw_bar0 = dev.bars[0];
            if raw_bar0 == 0 || raw_bar0 & 1 != 0 {
                continue;
            }
            let is_64bit = (raw_bar0 >> 1) & 0x03 == 0x02;
            let bar_phys = if is_64bit {
                ((raw_bar0 as u64) & !0x0F) | ((dev.bars[1] as u64) << 32)
            } else {
                (raw_bar0 as u64) & !0x0F
            };
            if bar_phys != 0 {
                crate::serial_println!(
                    "[audio] found HDA controller at {:02x}:{:02x}.{} BAR0={:#010x}",
                    dev.bus,
                    dev.device,
                    dev.function,
                    bar_phys
                );
                return Some(bar_phys);
            }
        }
    }
    None
}
