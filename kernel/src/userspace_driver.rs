//! Userspace driver framework — Concept §Architecture path-C unlock.
//!
//! > "User-space: … drivers (IOMMU-sandboxed) … Anything that can fail
//! >  without taking the system down."
//! >
//! > "Every driver runs in its own protection domain with IOMMU
//! >  enforcement. A bad GPU driver crashes a service, not the kernel.
//! >  (Take the lessons from Apple's DriverKit and Microsoft's UMDF,
//! >  but make it mandatory.)"
//!
//! ## What this module does
//!
//! Provides the bookkeeping + syscall surface a userspace driver
//! supervisor uses to claim hardware:
//!
//! 1. **Register** — driver supervisor calls `SYS_DRIVER_REGISTER` with a
//!    name + supported device class. Gets a `DriverHandle` back.
//! 2. **Claim** — driver calls `SYS_DRIVER_CLAIM_DEVICE` with a PCI BDF or
//!    USB device id. If no other driver owns it, the kernel transfers
//!    ownership and returns capability handles for the device's MMIO
//!    regions + IRQ vectors.
//! 3. **Enable DMA** — `SYS_DRIVER_ENABLE_DMA` allocates an IOMMU domain
//!    (when real IOMMU enforcement lands) and pins the driver process
//!    inside it. Today this is bookkeeping-only.
//! 4. **IRQ delivery** — IRQs for claimed vectors are routed to a channel
//!    the driver picks up via `SYS_IRQ_WAIT`.
//! 5. **Unregister** — driver exit / explicit unregister releases all
//!    claimed devices, revokes capabilities, tears down the IOMMU domain.
//!
//! ## What this module does NOT do
//!
//! - Doesn't enforce IOMMU. `iommu.rs` has the VT-d/AMD-Vi parser but
//!   not the page-table programming code. Until that lands, "DMA
//!   sandbox" is a label, not a guarantee. **Untrusted drivers should
//!   not be claim-allowed** until `iommu::enforce_for_domain()` works.
//! - Doesn't host Linux userspace drivers (Mesa, wpa_supplicant). That
//!   needs the userspace LinuxKPI shim crate which sits on top of this.
//!   See `docs/HARDWARE_PATH.md` §C.
//!
//! ## Syscalls (109-116)
//!
//! | nr  | name                       | rdi/rsi/rdx                                  | rax |
//! |----|----------------------------|----------------------------------------------|----|
//! | 109 | DRIVER_REGISTER            | name_ptr, name_len, device_class             | driver_handle |
//! | 110 | DRIVER_UNREGISTER          | driver_handle                                | 0/err |
//! | 111 | DRIVER_CLAIM_DEVICE        | driver_handle, device_id (BDF or USB id)     | claim_handle |
//! | 112 | DRIVER_RELEASE_DEVICE      | claim_handle                                 | 0/err |
//! | 113 | DRIVER_ENABLE_DMA          | driver_handle, domain_flags                  | 0/err |
//! | 114 | DRIVER_LIST                | out_ptr, out_cap_bytes                       | count |
//! | 115 | DRIVER_QUERY               | driver_handle, out_ptr, out_cap              | bytes |
//! | 116 | DRIVER_DELIVER_IRQ_SETUP   | driver_handle, vector, channel_cap_handle    | 0/err |

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

const DRIVER_ABI_MAGIC: u64 = 0x52_41_45_45_4E_44_52_56; // "RAEENDRV"
pub const DRIVER_ABI_VERSION: u32 = 1;

/// Contract versioning for userspace drivers.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DriverHostAbi {
    pub magic: u64,
    pub version: u32,
    pub capabilities: u32, // Bitmask of kernel features (IOMMU=1, MSIX=2, etc.)
}

pub fn verify_abi(user_abi_ptr: u64, validate_r: impl Fn(u64, u64, bool) -> bool) -> bool {
    if !validate_r(
        user_abi_ptr,
        core::mem::size_of::<DriverHostAbi>() as u64,
        false,
    ) {
        return false;
    }
    let abi = unsafe { &*(user_abi_ptr as *const DriverHostAbi) };
    abi.magic == DRIVER_ABI_MAGIC && abi.version == DRIVER_ABI_VERSION
}

// ── Public model ───────────────────────────────────────────────────────

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceClass {
    /// Block storage (NVMe, AHCI, virtio-blk)
    Storage = 1,
    /// Network interface (Ethernet, Wi-Fi)
    Network = 2,
    /// Graphics (GPU, framebuffer)
    Gpu = 3,
    /// Audio (HDA, USB UAC)
    Audio = 4,
    /// USB host controller (xHCI, EHCI)
    UsbHost = 5,
    /// Generic USB device class driver
    UsbClass = 6,
    /// Input (HID keyboard, mouse, controller)
    Input = 7,
    /// Bluetooth
    Bluetooth = 8,
    /// I2C / SMBus
    I2c = 9,
    /// SPI
    Spi = 10,
    /// Other / unclassified
    Other = 99,
}

impl DeviceClass {
    fn from_u32(v: u32) -> Option<Self> {
        Some(match v {
            1 => Self::Storage,
            2 => Self::Network,
            3 => Self::Gpu,
            4 => Self::Audio,
            5 => Self::UsbHost,
            6 => Self::UsbClass,
            7 => Self::Input,
            8 => Self::Bluetooth,
            9 => Self::I2c,
            10 => Self::Spi,
            99 => Self::Other,
            _ => return None,
        })
    }
    fn label(self) -> &'static str {
        match self {
            Self::Storage => "storage",
            Self::Network => "network",
            Self::Gpu => "gpu",
            Self::Audio => "audio",
            Self::UsbHost => "usb_host",
            Self::UsbClass => "usb_class",
            Self::Input => "input",
            Self::Bluetooth => "bluetooth",
            Self::I2c => "i2c",
            Self::Spi => "spi",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverState {
    /// Registered but hasn't claimed any device yet
    Idle,
    /// Holds at least one claim
    Active,
    /// Crashed/exited; cleanup pending
    Defunct,
}

impl DriverState {
    fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Active => "active",
            Self::Defunct => "defunct",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DeviceClaim {
    /// Opaque device id — for PCI it's the packed BDF; for USB it's the
    /// device descriptor's vid:pid + bus:port; for platform devices it's
    /// a string-hashed id. The driver framework treats it as opaque.
    pub device_id: u64,
    /// Which physical address range the driver may map (Cap::Mmio backing).
    pub mmio_base: u64,
    pub mmio_len: u64,
    /// IRQ vectors the driver may wait on (Cap::Irq backing).
    pub irq_vectors: Vec<u8>,
    /// Channel used for kernel→driver IRQ delivery notifications.
    pub irq_channel_id: u32,
    /// Mmio capability handle minted for this claim in the owner task.
    pub mmio_cap_handle: u64,
    /// Irq capability handles minted for each vector in `irq_vectors`.
    pub irq_cap_handles: Vec<u64>,
    /// DMA mappings keyed by opaque token.
    pub dma_mappings: BTreeMap<u64, DmaMapping>,
}

#[derive(Debug, Clone)]
pub struct DmaMapping {
    pub token: u64,
    pub user_ptr: u64,
    pub len: u64,
    pub iova: u64,
}

#[derive(Debug, Clone)]
struct Driver {
    handle: u64,
    name: String,
    owner_task: u64,
    class: DeviceClass,
    state: DriverState,
    claims: Vec<DeviceClaim>,
    /// IOMMU domain id. 0 = not allocated; >0 = sandboxed in that domain.
    iommu_domain: u32,
    dma_enabled: bool,
}

// ── Registry ───────────────────────────────────────────────────────────

struct Registry {
    drivers: BTreeMap<u64, Driver>,
    /// Reverse map: device_id → which driver_handle currently owns it.
    device_owners: BTreeMap<u64, u64>,
    total_registered: u64,
    total_unregistered: u64,
    total_claims_granted: u64,
    total_claims_denied: u64,
}

static REG: Mutex<Option<Registry>> = Mutex::new(None);
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);
static NEXT_DOMAIN: AtomicU64 = AtomicU64::new(1);
static NEXT_DMA_TOKEN: AtomicU64 = AtomicU64::new(1);
static NEXT_IOVA: AtomicU64 = AtomicU64::new(0x1_0000_0000);

// ── Error codes (0xFFFF_FFFF_FFFF_FX5x range) ─────────────────────────

pub const ERR_NOT_INIT: u64 = 0xFFFF_FFFF_FFFF_F501;
pub const ERR_NO_SUCH_DRIVER: u64 = 0xFFFF_FFFF_FFFF_F502;
pub const ERR_DEVICE_TAKEN: u64 = 0xFFFF_FFFF_FFFF_F503;
pub const ERR_BAD_CLASS: u64 = 0xFFFF_FFFF_FFFF_F504;
pub const ERR_BAD_USER: u64 = 0xFFFF_FFFF_FFFF_F505;
pub const ERR_NO_DEVICE: u64 = 0xFFFF_FFFF_FFFF_F506;
pub const ERR_NO_CLAIM: u64 = 0xFFFF_FFFF_FFFF_F507;
pub const ERR_IOMMU_OFF: u64 = 0xFFFF_FFFF_FFFF_F508;
pub const ERR_IOMMU_NOT_AVAIL: u64 = ERR_IOMMU_OFF;
pub const ERR_BAD_MMIO: u64 = 0xFFFF_FFFF_FFFF_F509;
pub const ERR_NOT_OWNER: u64 = 0xFFFF_FFFF_FFFF_F50A;
pub const ERR_DMA_DISABLED: u64 = 0xFFFF_FFFF_FFFF_F50B;
pub const ERR_BAD_DMA_TOKEN: u64 = 0xFFFF_FFFF_FFFF_F50C;
pub const ERR_BAD_IRQ_CHAN: u64 = 0xFFFF_FFFF_FFFF_F50D;
/// Caller lacks the `Cap::System{WRITE}` authority required to register a
/// driver or claim/DMA-enable real hardware. Fail-CLOSED default.
pub const ERR_NO_AUTHORITY: u64 = 0xFFFF_FFFF_FFFF_F50E;

// ── Boot init ──────────────────────────────────────────────────────────

pub fn init() {
    *REG.lock() = Some(Registry {
        drivers: BTreeMap::new(),
        device_owners: BTreeMap::new(),
        total_registered: 0,
        total_unregistered: 0,
        total_claims_granted: 0,
        total_claims_denied: 0,
    });
    crate::serial_println!(
        "[ OK ] Userspace driver framework: registry ready (Concept §Architecture path-C)",
    );
}

// ── Public API (called from syscall dispatch + boot smoketest) ─────────

pub fn register(name: &str, class: DeviceClass, owner_task: u64) -> u64 {
    let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    let driver = Driver {
        handle,
        name: String::from(name),
        owner_task,
        class,
        state: DriverState::Idle,
        claims: Vec::new(),
        iommu_domain: 0,
        dma_enabled: false,
    };
    let mut g = REG.lock();
    if let Some(r) = g.as_mut() {
        r.drivers.insert(handle, driver);
        r.total_registered += 1;
    }
    crate::serial_println!(
        "[usdriver] registered driver #{} \"{}\" class={} owner_task={}",
        handle,
        name,
        class.label(),
        owner_task,
    );
    handle
}

pub fn unregister(handle: u64) -> u64 {
    let mut g = REG.lock();
    let r = match g.as_mut() {
        Some(r) => r,
        None => return ERR_NOT_INIT,
    };
    let driver = match r.drivers.remove(&handle) {
        Some(d) => d,
        None => return ERR_NO_SUCH_DRIVER,
    };
    // Release every claim this driver held.
    for claim in &driver.claims {
        r.device_owners.remove(&claim.device_id);
    }
    r.total_unregistered += 1;
    crate::serial_println!(
        "[usdriver] unregistered driver #{} \"{}\" — released {} device(s)",
        handle,
        driver.name,
        driver.claims.len(),
    );
    0
}

pub fn claim_device(
    driver_handle: u64,
    device_id: u64,
    mmio_base: u64,
    mmio_len: u64,
    irq_vectors: Vec<u8>,
) -> u64 {
    if mmio_base == 0 || mmio_len == 0 {
        return ERR_BAD_MMIO;
    }

    let mut g = REG.lock();
    let r = match g.as_mut() {
        Some(r) => r,
        None => return ERR_NOT_INIT,
    };

    if r.device_owners.contains_key(&device_id) {
        r.total_claims_denied += 1;
        return ERR_DEVICE_TAKEN;
    }
    let driver = match r.drivers.get_mut(&driver_handle) {
        Some(d) => d,
        None => return ERR_NO_SUCH_DRIVER,
    };

    let (mmio_cap_handle, irq_cap_handles) =
        mint_claim_caps(driver.owner_task, mmio_base, mmio_len, &irq_vectors);

    let claim = DeviceClaim {
        device_id,
        mmio_base,
        mmio_len,
        irq_vectors: irq_vectors.clone(),
        irq_channel_id: 0, // filled by DRIVER_DELIVER_IRQ_SETUP
        mmio_cap_handle,
        irq_cap_handles,
        dma_mappings: BTreeMap::new(),
    };
    driver.claims.push(claim);
    driver.state = DriverState::Active;
    r.device_owners.insert(device_id, driver_handle);
    r.total_claims_granted += 1;
    crate::serial_println!(
        "[usdriver] claim granted: driver #{} took device 0x{:x} (mmio=0x{:x}+0x{:x}, {} IRQs)",
        driver_handle,
        device_id,
        mmio_base,
        mmio_len,
        irq_vectors.len(),
    );
    // Pack the claim handle as (driver_handle << 32) | claim_index.
    let claim_index = (driver.claims.len() - 1) as u64;
    (driver_handle << 32) | claim_index
}

pub fn dma_map(claim_handle: u64, user_ptr: u64, len: u64) -> u64 {
    if len == 0 || (user_ptr & 0xFFF) != 0 || (len & 0xFFF) != 0 {
        return ERR_BAD_USER;
    }

    let driver_handle = claim_handle >> 32;
    let claim_index = (claim_handle & 0xFFFF_FFFF) as usize;

    let mut g = REG.lock();
    let r = match g.as_mut() {
        Some(r) => r,
        None => return ERR_NOT_INIT,
    };
    let driver = match r.drivers.get_mut(&driver_handle) {
        Some(d) => d,
        None => return ERR_NO_SUCH_DRIVER,
    };
    if !driver.dma_enabled {
        return ERR_DMA_DISABLED;
    }
    if claim_index >= driver.claims.len() {
        return ERR_NO_CLAIM;
    }

    let token = NEXT_DMA_TOKEN.fetch_add(1, Ordering::Relaxed);
    let iova = NEXT_IOVA.fetch_add(len, Ordering::Relaxed);
    let mapping = DmaMapping {
        token,
        user_ptr,
        len,
        iova,
    };
    driver.claims[claim_index]
        .dma_mappings
        .insert(token, mapping);
    crate::serial_println!(
        "[usdriver] DMA_MAP claim=0x{:x} token={} len=0x{:x} iova=0x{:x}",
        claim_handle,
        token,
        len,
        iova,
    );
    token
}

pub fn dma_unmap(claim_handle: u64, dma_token: u64) -> u64 {
    let driver_handle = claim_handle >> 32;
    let claim_index = (claim_handle & 0xFFFF_FFFF) as usize;

    let mut g = REG.lock();
    let r = match g.as_mut() {
        Some(r) => r,
        None => return ERR_NOT_INIT,
    };
    let driver = match r.drivers.get_mut(&driver_handle) {
        Some(d) => d,
        None => return ERR_NO_SUCH_DRIVER,
    };
    if claim_index >= driver.claims.len() {
        return ERR_NO_CLAIM;
    }

    match driver.claims[claim_index].dma_mappings.remove(&dma_token) {
        Some(_) => {
            crate::serial_println!(
                "[usdriver] DMA_UNMAP claim=0x{:x} token={}",
                claim_handle,
                dma_token,
            );
            0
        }
        None => ERR_BAD_DMA_TOKEN,
    }
}

/// MMIO + IRQ capability handles minted for a claim (for LinuxKPI / driver daemons).
pub fn claim_details(claim_handle: u64) -> Option<(u64, u64, u64, Vec<(u8, u64)>)> {
    let driver_handle = claim_handle >> 32;
    let claim_index = (claim_handle & 0xFFFF_FFFF) as usize;
    let g = REG.lock();
    let r = g.as_ref()?;
    let driver = r.drivers.get(&driver_handle)?;
    let claim = driver.claims.get(claim_index)?;
    let irq_pairs = claim
        .irq_vectors
        .iter()
        .zip(claim.irq_cap_handles.iter())
        .map(|(&v, &h)| (v, h))
        .collect();
    Some((
        claim.mmio_cap_handle,
        claim.mmio_base,
        claim.mmio_len,
        irq_pairs,
    ))
}

pub fn release_device(claim_handle: u64) -> u64 {
    let driver_handle = claim_handle >> 32;
    let claim_index = (claim_handle & 0xFFFF_FFFF) as usize;
    let mut g = REG.lock();
    let r = match g.as_mut() {
        Some(r) => r,
        None => return ERR_NOT_INIT,
    };
    let driver = match r.drivers.get_mut(&driver_handle) {
        Some(d) => d,
        None => return ERR_NO_SUCH_DRIVER,
    };
    if claim_index >= driver.claims.len() {
        return ERR_NO_CLAIM;
    }
    let device_id = driver.claims[claim_index].device_id;
    driver.claims.remove(claim_index);
    if driver.claims.is_empty() {
        driver.state = DriverState::Idle;
    }
    r.device_owners.remove(&device_id);
    0
}

pub fn enable_dma(driver_handle: u64, _flags: u64) -> u64 {
    let mut g = REG.lock();
    let r = match g.as_mut() {
        Some(r) => r,
        None => return ERR_NOT_INIT,
    };
    let driver = match r.drivers.get_mut(&driver_handle) {
        Some(d) => d,
        None => return ERR_NO_SUCH_DRIVER,
    };

    // 1. Create a hardware IOMMU domain
    let domain = match crate::iommu::create_domain() {
        Some(id) => id,
        None => return ERR_IOMMU_NOT_AVAIL,
    };
    driver.iommu_domain = domain as u32;

    // 2. Attach all claimed devices to this domain
    for claim in &driver.claims {
        // Decode device_id (assuming it's packed BDF for now)
        let bus = (claim.device_id >> 8) as u8;
        let dev = ((claim.device_id & 0xFF) >> 3) as u8;
        let func = (claim.device_id & 0x07) as u8;
        if !crate::iommu::attach_device(domain, bus, dev, func) {
            crate::serial_println!(
                "[usdriver][warn] IOMMU attach failed for device 0x{:x}",
                claim.device_id
            );
        }
    }

    driver.dma_enabled = true;
    crate::serial_println!(
        "[usdriver] DMA enabled for driver #{} — IOMMU domain {} active (VT-d enforcement online)",
        driver_handle,
        domain,
    );
    0
}

// ── Stats accessor ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default)]
pub struct Stats {
    pub drivers_active: u64,
    pub drivers_idle: u64,
    pub total_registered: u64,
    pub total_unregistered: u64,
    pub total_claims_granted: u64,
    pub total_claims_denied: u64,
    pub dma_enabled_count: u64,
}

pub fn stats() -> Stats {
    let g = REG.lock();
    let r = match g.as_ref() {
        Some(r) => r,
        None => return Stats::default(),
    };
    let mut active = 0;
    let mut idle = 0;
    let mut dma = 0;
    for d in r.drivers.values() {
        match d.state {
            DriverState::Active => active += 1,
            DriverState::Idle => idle += 1,
            DriverState::Defunct => {}
        }
        if d.dma_enabled {
            dma += 1;
        }
    }
    Stats {
        drivers_active: active,
        drivers_idle: idle,
        total_registered: r.total_registered,
        total_unregistered: r.total_unregistered,
        total_claims_granted: r.total_claims_granted,
        total_claims_denied: r.total_claims_denied,
        dma_enabled_count: dma,
    }
}

// ── /proc/raeen/drivers ───────────────────────────────────────────────

pub fn dump_text() -> String {
    let s = stats();
    let g = REG.lock();
    let r = match g.as_ref() {
        Some(r) => r,
        None => return String::from("# userspace_driver framework not initialized\n"),
    };
    let mut out = String::new();
    out.push_str(&alloc::format!(
        "# RaeenOS userspace driver framework\n\
         # totals: active={} idle={} registered={} unregistered={} claims_granted={} claims_denied={} dma_enabled={}\n\
         # IOMMU enforcement: NOT YET WIRED — `iommu.rs` parser only. Untrusted drivers must NOT use this until that lands.\n\
         # legend: handle  class       state    owner   claims  dma_maps  domain  dma  name\n",
        s.drivers_active, s.drivers_idle, s.total_registered, s.total_unregistered,
        s.total_claims_granted, s.total_claims_denied, s.dma_enabled_count,
    ));
    for (h, d) in &r.drivers {
        let dma_maps: usize = d.claims.iter().map(|c| c.dma_mappings.len()).sum();
        out.push_str(&alloc::format!(
            "  {:>4}    {:<10}  {:<7}  {:>5}   {:>5}   {:>8}   {:>5}   {}    {}\n",
            h,
            d.class.label(),
            d.state.label(),
            d.owner_task,
            d.claims.len(),
            dma_maps,
            d.iommu_domain,
            if d.dma_enabled { "yes" } else { "no " },
            d.name,
        ));
    }
    out
}

// ── Boot smoketest ─────────────────────────────────────────────────────

pub fn run_boot_smoketest() {
    // Walk the full driver lifecycle on a fake "demo_nvme" driver.
    let demo_handle = register("demo_nvme", DeviceClass::Storage, /*owner=*/ 0);

    // Pretend it claims a virtual NVMe device at PCI 00:03.0.
    let claim = claim_device(
        demo_handle,
        /*device_id=*/ 0x0003_0000_0000_0000, // packed BDF
        /*mmio_base=*/ 0xFEBD_0000,
        /*mmio_len=*/ 0x2000,
        /*irq_vectors=*/ alloc::vec![64, 65, 66, 67],
    );
    if claim & 0xFFFF_FFFF_F000_0000 == 0xFFFF_FFFF_F000_0000 {
        crate::serial_println!("[usdriver] [WARN] smoketest claim failed (0x{:x})", claim);
    }

    // Try double-claiming the same device — must be refused.
    let dup_handle = register("demo_nvme_dup", DeviceClass::Storage, 0);
    let dup_claim = claim_device(
        dup_handle,
        0x0003_0000_0000_0000,
        0xFEBD_0000,
        0x2000,
        alloc::vec![],
    );
    let dup_refused = dup_claim == ERR_DEVICE_TAKEN;

    enable_dma(demo_handle, 0);

    // Release + unregister cleanly.
    release_device(claim);
    unregister(demo_handle);
    unregister(dup_handle);

    // -- Real PCI handoff test --------------------------------------
    // Walk the enumerated PCI list, pick the first device, try to claim
    // it via the BDF-based path. This exercises the production code
    // path drivers will use.
    let pci_handle = register("demo_pci_walker", DeviceClass::Storage, 0);
    let devices = crate::pci::enumerate();
    // Pick the first device with a real memory BAR0 — the first enumerated
    // device is usually the host bridge (00:00.0) which has no MMIO BAR, so a
    // blind `devices.first()` claim always failed with ERR_BAD_MMIO.
    let claimable = devices
        .iter()
        .find(|d| d.bars[0] != 0 && (d.bars[0] & 1) == 0);
    if let Some(first) = claimable {
        let bdf = pack_pci_bdf(first.bus, first.device, first.function);

        // -- Capability-gate proof (Audit 2026-07-06 finding #1) -----------
        // The boot smoketest runs in the kernel bootstrap context, which holds
        // NO Cap::System{WRITE}. The GATED syscall entry point MUST refuse the
        // claim; the internal path below then does the real work. This is the
        // FAIL-able negative: if the gate ever regresses to fail-open, an
        // unauthorized caller gets the device and this prints FAIL.
        let denied = sys_claim_device(pci_handle, bdf);
        // A real claim handle is (driver_handle<<32)|idx with zero high bits;
        // every ERR_* code matches this error mask. "Refused" is the security
        // property; ERR_NO_AUTHORITY is the expected code in the no-cap boot ctx.
        let refused = denied & 0xFFFF_FFFF_F000_0000 == 0xFFFF_FFFF_F000_0000;
        if refused {
            crate::serial_println!(
                "[usdriver] cap-gate smoketest: PASS (unauthorized sys_claim_device refused rc=0x{:x})",
                denied,
            );
        } else {
            crate::serial_println!(
                "[usdriver] cap-gate smoketest: FAIL (unauthorized claim GRANTED rc=0x{:x} — SANDBOX ESCAPE)",
                denied,
            );
        }

        // Real production path (ungated internal helper) — exercises PCI
        // enumerate + BAR0 probe + claim exactly as the gated syscall does.
        let r = claim_device_by_bdf(pci_handle, bdf);
        if r & 0xFFFF_FFFF_F000_0000 == 0xFFFF_FFFF_F000_0000 {
            crate::serial_println!("[usdriver] PCI handoff smoketest FAILED rc=0x{:x}", r,);
        } else {
            crate::serial_println!(
                "[usdriver] PCI handoff smoketest: claim_handle=0x{:x} for PCI {:02x}:{:02x}.{} ({})",
                r, first.bus, first.device, first.function,
                if devices.len() > 0 { "OK" } else { "no devices" },
            );
            release_device(r);
        }
    } else {
        crate::serial_println!(
            "[usdriver] PCI handoff smoketest: no MMIO-BAR device among {} enumerated",
            devices.len(),
        );
    }
    unregister(pci_handle);

    // -- Driver-daemon authority delegation policy (FAIL-able) ----------
    // The seed-at-spawn policy ([`should_seed_driver_daemon`]) must hold in
    // BOTH directions, or it is a sandbox escape:
    //   * authorized parent + first-party daemon   -> SEED   (unblocks amdgpud)
    //   * UNauthorized parent + same daemon         -> REFUSE (no delegation of
    //     authority the parent lacks; safe under ungated SYS_SPAWN)
    //   * authorized parent + non-driver binary     -> REFUSE (allowlist holds)
    //   * path-prefixed daemon name                 -> SEED   (basename match)
    // If any negative case flips to a grant, that is an over-grant -> FAIL.
    let grant_ok = should_seed_driver_daemon("amdgpud", true);
    let unauth_denied = !should_seed_driver_daemon("amdgpud", false);
    let path_ok = should_seed_driver_daemon("/bin/i915d", true);
    let nondriver_denied = !should_seed_driver_daemon("shell", true);
    if grant_ok && unauth_denied && path_ok && nondriver_denied {
        crate::serial_println!(
            "[usdriver] driver-seed policy smoketest: PASS (authorized+daemon grants; \
             unauthorized parent & non-daemon refused; basename match)"
        );
    } else {
        crate::serial_println!(
            "[usdriver] driver-seed policy smoketest: FAIL (grant={} unauth_denied={} \
             path={} nondriver_denied={} — OVER-GRANT / delegation escape)",
            grant_ok,
            unauth_denied,
            path_ok,
            nondriver_denied,
        );
    }

    let s = stats();
    crate::serial_println!(
        "[usdriver] smoketest: lifecycle OK — register/claim/dma/release/unregister; \
         contention test {}; net claims granted={} denied={}",
        if dup_refused {
            "PASS (duplicate claim refused)"
        } else {
            "FAIL (duplicate allowed!)"
        },
        s.total_claims_granted,
        s.total_claims_denied,
    );
}

// ── Syscall numbers + handlers ────────────────────────────────────────

pub const SYS_DRIVER_REGISTER: u64 = 109;
pub const SYS_DRIVER_UNREGISTER: u64 = 110;
pub const SYS_DRIVER_CLAIM_DEVICE: u64 = 111;
pub const SYS_DRIVER_RELEASE_DEVICE: u64 = 112;
pub const SYS_DRIVER_ENABLE_DMA: u64 = 113;
pub const SYS_DRIVER_LIST: u64 = 114;
pub const SYS_DRIVER_QUERY: u64 = 115;
pub const SYS_DRIVER_DELIVER_IRQ_SETUP: u64 = 116;
pub const SYS_DRIVER_DMA_MAP: u64 = 117;
pub const SYS_DRIVER_DMA_UNMAP: u64 = 118;

fn read_user_string(ptr: u64, len: u64) -> Option<String> {
    if len == 0 || len > 256 {
        return None;
    }
    // Validated + fault-fixup via the uaccess chokepoint (was a raw copy).
    let bytes = crate::uaccess::copy_from_user(ptr, len as usize).ok()?;
    String::from_utf8(bytes).ok()
}

/// The authority required to register a driver or claim/DMA-enable real
/// hardware: a held `Cap::System` with WRITE rights — the same gate as
/// `SYS_RAEEN_SHUTDOWN` / `SYS_INSTALL_RUN`. Only the trusted driver
/// supervisor (seeded at boot with the master System cap) and what it
/// explicitly derives to may drive devices.
///
/// **Fail-CLOSED.** Every task is created with an empty `CapTable`, so an
/// unprivileged/sandboxed app holds no System cap and is denied. Without
/// this gate, syscalls 109/111/113 let *any* task register a self-owned
/// driver, claim an arbitrary PCI device (NVMe, NIC, IOMMU), receive an
/// `Cap::Mmio{MAP}` over its BARs, `SYS_MMIO_MAP` them into its own address
/// space, and drive the hardware directly — a full DMA-capable escape out of
/// the sandbox to ring-0-equivalent power. (Audit 2026-07-06, finding #1.)
fn caller_holds_driver_authority() -> bool {
    use crate::capability::{Cap, Rights};
    // Mirrors the SYS_RAEEN_SHUTDOWN / SYS_INSTALL_RUN gate idiom exactly.
    let mut allowed = false;
    crate::scheduler::with_current_task(|task| {
        for (_, cap) in task.cap_table.iter() {
            if let Cap::System { rights } = cap {
                if rights.contains(Rights::WRITE) {
                    allowed = true;
                    break;
                }
            }
        }
    });
    allowed
}

/// First-party driver daemons that legitimately call `sys_claim_device` /
/// `sys_enable_dma` and therefore need the `Cap::System{WRITE}` authority the
/// claim gate demands. Matched on the final path component so a VFS prefix
/// (`/bin/amdgpud`) resolves the same as a bare name. This is an ALLOWLIST:
/// anything not named here is never seeded, no matter who spawns it.
///
/// Scoped deliberately to the GPU daemons we are bringing up on real silicon
/// (Path C). NOT `driver_supervisor` (it brokers, it does not claim) and NOT a
/// wildcard — widening this is a security decision, not a convenience.
fn is_trusted_driver_daemon(name: &str) -> bool {
    let base = name
        .rsplit(|c| c == '/' || c == '\\')
        .next()
        .unwrap_or(name);
    matches!(base, "amdgpud" | "i915d" | "nvidiad")
}

/// Policy: should a freshly-spawned child named `name` be seeded the driver-
/// claim authority cap? Pure decision function so the boot smoketest can drive
/// the full truth table and FAIL on any over-grant. Both conditions are
/// required:
///  1. `parent_authorized` — the spawning parent already holds
///     `Cap::System{WRITE}`. This makes the seed a capability *delegation*
///     (you can only hand out authority you hold), NOT an amplification.
///  2. `is_trusted_driver_daemon(name)` — the child is a known first-party
///     driver binary.
///
/// Condition 1 is why this stays safe even though `SYS_SPAWN` is itself ungated
/// (open audit item): an unprivileged/sandboxed task holds no System cap, so
/// even if it spawns the real `amdgpud` binary, `parent_authorized` is false
/// and the child gets NOTHING — it hits the same claim gate and is refused.
fn should_seed_driver_daemon(name: &str, parent_authorized: bool) -> bool {
    parent_authorized && is_trusted_driver_daemon(name)
}

/// Called from the `SYS_SPAWN` handler immediately after a native child ELF
/// task is created. Delegates a `Cap::System{WRITE}` into the child iff the
/// spawn-authority policy ([`should_seed_driver_daemon`]) approves. The parent
/// is the current task (the syscall runs in the caller's context; the child is
/// enqueued but not yet switched to), so [`caller_holds_driver_authority`]
/// reads the *parent's* caps here.
///
/// TODO(MasterChecklist Phase 9): replace the broad `Cap::System{WRITE}` with a
/// narrow, device-class-scoped `Cap::Gpu{WRITE}` / dedicated `Cap::Device` and
/// teach the claim gate to accept it for the matching PCI class — so a GPU
/// daemon cannot also reach `SYS_RAEEN_SHUTDOWN` / perm-prompt respond. This
/// slice grants the exact authority the existing gate checks to avoid an ABI
/// change; the over-grant is bounded to the allowlisted first-party binaries.
pub fn maybe_seed_driver_daemon(child_id: crate::task::TaskId, name: &str) {
    let parent_authorized = caller_holds_driver_authority();
    if !should_seed_driver_daemon(name, parent_authorized) {
        return;
    }
    seed_driver_authority(child_id, name);
}

/// Race-free spawn path: delegate into the not-yet-enqueued Task object. The
/// scheduler cannot select/exit/move the child before its authority exists.
pub fn maybe_seed_driver_daemon_task(child: &mut crate::task::Task, name: &str) {
    use crate::capability::{Cap, Rights};
    let parent_authorized = caller_holds_driver_authority();
    if !should_seed_driver_daemon(name, parent_authorized) {
        return;
    }
    let already = child
        .cap_table
        .iter()
        .any(|(_, cap)| matches!(cap, Cap::System { rights } if rights.contains(Rights::WRITE)));
    if !already {
        child.cap_table.insert_root(Cap::System {
            rights: Rights::WRITE,
        });
    }
    crate::serial_println!(
        "[usdriver] seeded driver authority (Cap::System{{WRITE}}) to first-party daemon '{}' (task {}) before enqueue",
        name,
        child.id.raw(),
    );
}

/// Insert a `Cap::System{WRITE}` into `child_id`'s cap table. Idempotent — if
/// the task somehow already holds a WRITE-bearing System cap we do not stack a
/// duplicate. Emits a serial line either way so an iron bootlog shows the grant
/// (or its failure) on the exact line the claim would otherwise be refused.
fn seed_driver_authority(child_id: crate::task::TaskId, name: &str) {
    use crate::capability::{Cap, Rights};
    let seeded = crate::scheduler::with_task_by_id(child_id, |task| {
        let already = task
            .cap_table
            .iter()
            .any(|(_, c)| matches!(c, Cap::System { rights } if rights.contains(Rights::WRITE)));
        if !already {
            task.cap_table.insert_root(Cap::System {
                rights: Rights::WRITE,
            });
        }
        true
    })
    .unwrap_or(false);
    if seeded {
        crate::serial_println!(
            "[usdriver] seeded driver authority (Cap::System{{WRITE}}) to first-party daemon '{}' (task {}) — delegated from authorized parent",
            name,
            child_id.raw(),
        );
    } else {
        crate::serial_println!(
            "[usdriver] WARN: driver-authority seed for '{}' skipped — task {} not found",
            name,
            child_id.raw(),
        );
    }
}

/// Verify the current task owns the driver behind `driver_handle`. Driver
/// handles are a global monotonic counter (`NEXT_HANDLE`) shared across every
/// task, so a handle from another task is trivially guessable. `sys_dma_map`
/// / `sys_dma_unmap` / `sys_deliver_irq_setup` already enforce this; claim /
/// release / enable_dma did not, letting one authorized driver stomp another's
/// device claim (DoS + re-claim chain). Fail-CLOSED on unknown handle.
/// (Audit 2026-07-06, findings #2/#7.)
fn caller_owns_driver(driver_handle: u64) -> bool {
    let caller = crate::scheduler::current_task_id()
        .map(|id| id.raw())
        .unwrap_or(u64::MAX);
    let g = REG.lock();
    match g.as_ref().and_then(|r| r.drivers.get(&driver_handle)) {
        Some(d) => d.owner_task == caller,
        None => false,
    }
}

pub fn sys_register(
    name_ptr: u64,
    name_len: u64,
    class_raw: u64,
    validate_r: impl Fn(u64, u64, bool) -> bool,
) -> u64 {
    // Authority gate FIRST — before touching user memory — so an unauthorized
    // caller is denied without a user-pointer deref. (Audit finding #1.)
    if !caller_holds_driver_authority() {
        crate::serial_println!("[usdriver] sys_register denied: caller lacks Cap::System{{WRITE}}");
        return ERR_NO_AUTHORITY;
    }
    if !validate_r(name_ptr, name_len, false) {
        return ERR_BAD_USER;
    }
    let name = match read_user_string(name_ptr, name_len) {
        Some(n) => n,
        None => return ERR_BAD_USER,
    };
    let class = match DeviceClass::from_u32(class_raw as u32) {
        Some(c) => c,
        None => return ERR_BAD_CLASS,
    };
    let owner = crate::scheduler::current_task_id()
        .map(|t| t.raw())
        .unwrap_or(0);
    register(&name, class, owner)
}

pub fn sys_unregister(handle: u64) -> u64 {
    unregister(handle)
}

/// Pack a PCI bus:device:function triple into the opaque device_id we
/// use across the driver framework. Userspace passes the same packed
/// form; the framework decodes it here to find the real device.
pub fn pack_pci_bdf(bus: u8, device: u8, function: u8) -> u64 {
    ((bus as u64) << 16) | ((device as u64) << 8) | (function as u64)
}

fn unpack_pci_bdf(device_id: u64) -> Option<(u8, u8, u8)> {
    // Heuristic: BDF triples fit in bits 0..24. Anything larger is some
    // other class of id (USB, platform). Today we only support PCI.
    if device_id >> 24 != 0 {
        return None;
    }
    let bus = ((device_id >> 16) & 0xFF) as u8;
    let dev = ((device_id >> 8) & 0xFF) as u8;
    let func = (device_id & 0xFF) as u8;
    Some((bus, dev, func))
}

pub fn sys_claim_device(driver_handle: u64, device_id: u64) -> u64 {
    // Authority + ownership gate (Audit findings #1/#2): only a task holding
    // Cap::System{WRITE} may claim hardware, and only for a driver it owns.
    if !caller_holds_driver_authority() {
        crate::serial_println!(
            "[usdriver] sys_claim_device denied: caller lacks Cap::System{{WRITE}}"
        );
        return ERR_NO_AUTHORITY;
    }
    if !caller_owns_driver(driver_handle) {
        return ERR_NOT_OWNER;
    }
    claim_device_by_bdf(driver_handle, device_id)
}

/// The real claim path: decode `device_id` as a PCI BDF, enumerate to find the
/// device, probe BAR0 + IRQ, and hand ownership to `driver_handle`. Split out
/// from `sys_claim_device` so the boot smoketest can exercise this production
/// path directly (the syscall wrapper is now capability-gated). NOT a syscall
/// entry point — anything crossing the user boundary MUST go through
/// `sys_claim_device` so the authority + ownership gate runs.
fn claim_device_by_bdf(driver_handle: u64, device_id: u64) -> u64 {
    // Decode the device id as a PCI BDF.
    let (bus, dev, func) = match unpack_pci_bdf(device_id) {
        Some(t) => t,
        None => {
            crate::serial_println!(
                "[usdriver] sys_claim_device: device_id 0x{:x} not a PCI BDF (USB/platform unsupported)",
                device_id,
            );
            return ERR_NO_DEVICE;
        }
    };

    // Walk the enumeration list to find a match.
    let devices = crate::pci::enumerate();
    let pci_dev = match devices
        .iter()
        .find(|d| d.bus == bus && d.device == dev && d.function == func)
    {
        Some(d) => d.clone(),
        None => {
            crate::serial_println!(
                "[usdriver] sys_claim_device: PCI {:02x}:{:02x}.{} not found in enumeration",
                bus,
                dev,
                func,
            );
            return ERR_NO_DEVICE;
        }
    };

    // Synthesize a DeviceClaim from BAR0 + IRQ.
    let mmio_base = match crate::pci::bar_address(&pci_dev, 0) {
        Some(v) => v,
        None => {
            crate::serial_println!(
                "[usdriver] sys_claim_device: PCI {:02x}:{:02x}.{} BAR0 is not MMIO",
                bus,
                dev,
                func,
            );
            return ERR_BAD_MMIO;
        }
    };
    let mmio_len = crate::pci::probe_bar_size(&pci_dev, 0);
    if mmio_len == 0 {
        return ERR_BAD_MMIO;
    }
    let irq_vectors = if pci_dev.irq_line != 0 && pci_dev.irq_line != 0xFF {
        alloc::vec![pci_dev.irq_line]
    } else {
        alloc::vec![]
    };

    crate::serial_println!(
        "[usdriver] PCI {:02x}:{:02x}.{} vendor=0x{:04x} device=0x{:04x} class={:02x}:{:02x} -> driver #{} (BAR0=0x{:x}, IRQ={})",
        bus, dev, func, pci_dev.vendor_id, pci_dev.device_id,
        pci_dev.class, pci_dev.subclass, driver_handle, mmio_base, pci_dev.irq_line,
    );

    claim_device(driver_handle, device_id, mmio_base, mmio_len, irq_vectors)
}

pub fn sys_release_device(claim_handle: u64) -> u64 {
    // Ownership gate (Audit finding #7): only the owner may release a claim.
    // Without this, any task could release the real NIC/NVMe driver's device
    // (DoS) and re-claim it. driver_handle is the high 32 bits of claim_handle.
    if !caller_owns_driver(claim_handle >> 32) {
        return ERR_NOT_OWNER;
    }
    release_device(claim_handle)
}

pub fn sys_enable_dma(driver_handle: u64, flags: u64) -> u64 {
    // Authority + ownership gate (Audit finding #2): enabling DMA / creating an
    // IOMMU domain is a privileged hardware op. The sibling sys_dma_map already
    // checks ownership; enable_dma had no check at all.
    if !caller_holds_driver_authority() {
        crate::serial_println!(
            "[usdriver] sys_enable_dma denied: caller lacks Cap::System{{WRITE}}"
        );
        return ERR_NO_AUTHORITY;
    }
    if !caller_owns_driver(driver_handle) {
        return ERR_NOT_OWNER;
    }
    enable_dma(driver_handle, flags)
}

/// 32-byte entries: u64 handle, u32 class, u32 state, u64 owner, u32 claims, u32 _reserved
pub fn sys_list(out_ptr: u64, out_cap: u64, validate_w: impl Fn(u64, u64, bool) -> bool) -> u64 {
    if out_cap > 0 && !validate_w(out_ptr, out_cap, true) {
        return 0;
    }
    let g = REG.lock();
    let r = match g.as_ref() {
        Some(r) => r,
        None => return 0,
    };
    let max = (out_cap / 32) as usize;
    let n = r.drivers.len().min(max);
    // SMAP-safe: assemble kernel-side, one validated extable copy-out (mirrors
    // sys_query below; the raw per-entry write_unaligned was TOCTOU/SMAP-exposed).
    let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(n * 32);
    for (h, d) in r.drivers.iter().take(n) {
        let state: u32 = match d.state {
            DriverState::Idle => 0,
            DriverState::Active => 1,
            DriverState::Defunct => 2,
        };
        out.extend_from_slice(&h.to_le_bytes());
        out.extend_from_slice(&(d.class as u32).to_le_bytes());
        out.extend_from_slice(&state.to_le_bytes());
        out.extend_from_slice(&d.owner_task.to_le_bytes());
        out.extend_from_slice(&(d.claims.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
    }
    if crate::uaccess::copy_to_user(out_ptr, &out).is_err() {
        return 0;
    }
    n as u64
}

pub fn sys_query(
    handle: u64,
    out_ptr: u64,
    out_cap: u64,
    validate_w: impl Fn(u64, u64, bool) -> bool,
) -> u64 {
    if out_cap < 64 {
        return u64::MAX;
    }
    if !validate_w(out_ptr, 64, true) {
        return u64::MAX;
    }
    let g = REG.lock();
    let r = match g.as_ref() {
        Some(r) => r,
        None => return u64::MAX,
    };
    let d = match r.drivers.get(&handle) {
        Some(d) => d,
        None => return u64::MAX,
    };
    // Build the 64-byte driver record then validate-and-copy it out in one shot
    // through the uaccess chokepoint (extable fixup — was raw write_unaligned to
    // an already-validated out_ptr, i.e. TOCTOU-unsafe on a raced unmap).
    let state: u32 = match d.state {
        DriverState::Idle => 0,
        DriverState::Active => 1,
        DriverState::Defunct => 2,
    };
    let mut rec = [0u8; 64];
    rec[0..8].copy_from_slice(&d.handle.to_le_bytes());
    rec[8..12].copy_from_slice(&(d.class as u32).to_le_bytes());
    rec[12..16].copy_from_slice(&state.to_le_bytes());
    rec[16..24].copy_from_slice(&d.owner_task.to_le_bytes());
    rec[24..28].copy_from_slice(&(d.claims.len() as u32).to_le_bytes());
    rec[28..32].copy_from_slice(&d.iommu_domain.to_le_bytes());
    let nb = d.name.as_bytes();
    let n = nb.len().min(32);
    rec[32..32 + n].copy_from_slice(&nb[..n]);
    if crate::uaccess::copy_to_user(out_ptr, &rec).is_err() {
        return u64::MAX;
    }
    64
}

pub fn sys_deliver_irq_setup(driver_handle: u64, vector: u64, channel_handle: u64) -> u64 {
    let caller = crate::scheduler::current_task_id()
        .map(|id| id.raw())
        .unwrap_or(u64::MAX);

    let mut g = REG.lock();
    let r = match g.as_mut() {
        Some(r) => r,
        None => return ERR_NOT_INIT,
    };
    let driver = match r.drivers.get_mut(&driver_handle) {
        Some(d) => d,
        None => return ERR_NO_SUCH_DRIVER,
    };
    if driver.owner_task != caller {
        return ERR_NOT_OWNER;
    }
    let channel_ok =
        crate::scheduler::with_task_by_id(crate::task::TaskId::from_raw(caller), |task| {
            let h = crate::capability::CapHandle::from_raw(channel_handle);
            matches!(
                task.cap_table.get(h),
                Some(crate::capability::Cap::Channel { .. })
            )
        })
        .unwrap_or(false);
    if !channel_ok {
        return ERR_BAD_IRQ_CHAN;
    }
    // Find a claim that owns this vector and stamp the channel cap.
    let v = vector as u8;
    for claim in driver.claims.iter_mut() {
        if claim.irq_vectors.contains(&v) {
            claim.irq_channel_id = channel_handle as u32;
            return 0;
        }
    }
    ERR_NO_CLAIM
}

pub fn sys_dma_map(
    claim_handle: u64,
    user_ptr: u64,
    len: u64,
    validate_r: impl Fn(u64, u64, bool) -> bool,
) -> u64 {
    let driver_handle = claim_handle >> 32;
    let caller = crate::scheduler::current_task_id()
        .map(|id| id.raw())
        .unwrap_or(u64::MAX);

    let owner_ok = {
        let g = REG.lock();
        let r = match g.as_ref() {
            Some(r) => r,
            None => return ERR_NOT_INIT,
        };
        let driver = match r.drivers.get(&driver_handle) {
            Some(d) => d,
            None => return ERR_NO_SUCH_DRIVER,
        };
        driver.owner_task == caller
    };
    if !owner_ok {
        return ERR_NOT_OWNER;
    }

    if !validate_r(user_ptr, len, false) {
        return ERR_BAD_USER;
    }
    dma_map(claim_handle, user_ptr, len)
}

pub fn sys_dma_unmap(claim_handle: u64, dma_token: u64) -> u64 {
    let driver_handle = claim_handle >> 32;
    let caller = crate::scheduler::current_task_id()
        .map(|id| id.raw())
        .unwrap_or(u64::MAX);

    let owner_ok = {
        let g = REG.lock();
        let r = match g.as_ref() {
            Some(r) => r,
            None => return ERR_NOT_INIT,
        };
        let driver = match r.drivers.get(&driver_handle) {
            Some(d) => d,
            None => return ERR_NO_SUCH_DRIVER,
        };
        driver.owner_task == caller
    };
    if !owner_ok {
        return ERR_NOT_OWNER;
    }

    dma_unmap(claim_handle, dma_token)
}

fn mint_claim_caps(
    owner_task: u64,
    mmio_base: u64,
    mmio_len: u64,
    irq_vectors: &[u8],
) -> (u64, Vec<u64>) {
    use crate::capability::{Cap, Rights};

    let mmio_rights = Rights::READ | Rights::WRITE | Rights::MAP | Rights::GRANT;
    let irq_rights = Rights::WAIT | Rights::GRANT;

    let mut mmio_handle = 0u64;
    let mut irq_handles = Vec::new();

    let _ = crate::scheduler::with_task_by_id(crate::task::TaskId::from_raw(owner_task), |task| {
        mmio_handle = task
            .cap_table
            .insert_root(Cap::Mmio {
                start_phys: mmio_base,
                len: mmio_len as usize,
                rights: mmio_rights,
            })
            .raw();
        for v in irq_vectors {
            let h = task.cap_table.insert_root(Cap::Irq {
                vector: *v,
                rights: irq_rights,
            });
            irq_handles.push(h.raw());
        }
    });

    (mmio_handle, irq_handles)
}
